//! What the service asks of its host: logging, the viewer's statistics,
//! the proxy settings applied to each libcurl handle (`LLProxy`), the SSL
//! context callback (certificate verification against the viewer's store),
//! thread registration and the release of caller data. The viewer supplies
//! these through the C ABI; the crate's tests use [`NoHooks`].

use std::ffi::c_void;

use crate::sys::{CURL, SslInfo, UserPtr};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LogLevel {
    Debug = 0,
    Info = 1,
    Warn = 2,
}

/// The viewer's `HTTPStats` records.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stat {
    DataDown = 0,
    DataUp = 1,
    ResultCode = 2,
}

pub trait Hooks: Send + Sync {
    /// A log line of the "CoreHttp" tag.
    fn log(&self, level: LogLevel, message: &str);

    /// A statistics record.
    fn stat(&self, stat: Stat, value: u64);

    /// Applies the viewer's proxy settings to a handle about to be used
    /// (`LLProxy::applyProxySettings`). Called when the `PO_LLPROXY` global
    /// option is set, in place of the library's own `PO_HTTP_PROXY`.
    fn apply_proxy(&self, curl: *mut CURL);

    /// The `CURLOPT_SSL_CTX_FUNCTION` body: installs the host's certificate
    /// verification on the OpenSSL context. Called when the
    /// `PO_SSL_VERIFY_CALLBACK` option is set. The verification callback it
    /// installs writes its decision into `info`.
    fn ssl_ctx(&self, ssl_ctx: *mut c_void, info: &mut SslInfo);

    /// The service thread has started (register it with the host).
    fn thread_start(&self) {}

    /// The service thread is about to exit.
    fn thread_stop(&self) {}

    /// Caller data of an operation that will never be delivered.
    fn release_user(&self, _user: UserPtr) {}
}

/// Hooks that do nothing: no proxy, no certificate callback, no logging.
#[derive(Default, Debug)]
pub struct NoHooks;

impl Hooks for NoHooks {
    fn log(&self, _level: LogLevel, _message: &str) {}
    fn stat(&self, _stat: Stat, _value: u64) {}
    fn apply_proxy(&self, _curl: *mut CURL) {}
    fn ssl_ctx(&self, _ssl_ctx: *mut c_void, _info: &mut SslInfo) {}
}

/// Hooks that print log lines to stderr (for the crate's tests).
#[derive(Default, Debug)]
pub struct StderrHooks;

impl Hooks for StderrHooks {
    fn log(&self, level: LogLevel, message: &str) {
        eprintln!("[vkstorm-http {level:?}] {message}");
    }
    fn stat(&self, _stat: Stat, _value: u64) {}
    fn apply_proxy(&self, _curl: *mut CURL) {}
    fn ssl_ctx(&self, _ssl_ctx: *mut c_void, _info: &mut SslInfo) {}
}
