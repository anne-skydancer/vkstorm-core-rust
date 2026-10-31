//! Request options (`HttpOptions`), the per-class and global policy options
//! (`HttpPolicyClass`, `HttpPolicyGlobal`), the option descriptor table of
//! `HttpService` and the limits of `_httpinternal.h`.

use crate::status::{Status, he};

pub const POLICY_CLASS_LIMIT: usize = 32;

pub const TRACE_OFF: i32 = 0;
pub const TRACE_LOW: i32 = 1;
pub const TRACE_CURL_HEADERS: i32 = 2;
pub const TRACE_CURL_BODIES: i32 = 3;
pub const TRACE_MIN: i32 = TRACE_OFF;
pub const TRACE_MAX: i32 = TRACE_CURL_BODIES;

pub const RETRY_COUNT_DEFAULT: u32 = 5;
pub const RETRY_COUNT_MIN: u32 = 0;
pub const RETRY_COUNT_MAX: u32 = 100;
/// Microseconds.
pub const RETRY_BACKOFF_MIN_DEFAULT: u64 = 1_000_000;
/// Microseconds. The C++ constant reads "5 sec" but is 50 000 006.
pub const RETRY_BACKOFF_MAX_DEFAULT: u64 = 50_000_006;
pub const RETRY_BACKOFF_MAX: u64 = 20_000_000;
pub const REDIRECTS_DEFAULT: i64 = 10;

pub const REQUEST_TIMEOUT_DEFAULT: u32 = 30;
pub const REQUEST_XFER_TIMEOUT_DEFAULT: u32 = 0;
pub const REQUEST_TIMEOUT_MIN: u32 = 0;
pub const REQUEST_TIMEOUT_MAX: u32 = 3600;

pub const CONNECTION_LIMIT_DEFAULT: i64 = 8;
pub const CONNECTION_LIMIT_MIN: i64 = 1;
pub const CONNECTION_LIMIT_MAX: i64 = 256;

pub const PIPELINING_DEFAULT: i64 = 0;
pub const PIPELINING_MAX: i64 = 20;

pub const USE_RETRY_AFTER_DEFAULT: bool = true;
pub const THROTTLE_RATE_DEFAULT: i64 = 0;

/// Milliseconds the service thread sleeps between passes when polling.
pub const SERVICE_LOOP_SLEEP_NORMAL_MS: u64 = 2;

pub const DEFAULT_POLICY_ID: u32 = 0;
pub const INVALID_POLICY_ID: u32 = 0xFFFF_FFFF;
pub const GLOBAL_POLICY_ID: u32 = 0xFFFF_FFFE;

/// Per-request options, the values of a C++ `HttpOptions` at submission.
#[derive(Clone, Debug, PartialEq)]
pub struct RequestOptions {
    pub want_headers: bool,
    pub trace: i32,
    pub timeout: u32,
    pub transfer_timeout: u32,
    pub retries: u32,
    pub min_backoff: u64,
    pub max_backoff: u64,
    pub use_retry_after: bool,
    pub follow_redirects: bool,
    pub verify_peer: bool,
    pub verify_host: bool,
    pub dns_cache_timeout: i32,
    pub headers_only: bool,
    pub last_modified: i64,
}

impl Default for RequestOptions {
    fn default() -> Self {
        RequestOptions {
            want_headers: false,
            trace: TRACE_OFF,
            timeout: REQUEST_TIMEOUT_DEFAULT,
            transfer_timeout: REQUEST_XFER_TIMEOUT_DEFAULT,
            retries: RETRY_COUNT_DEFAULT,
            min_backoff: RETRY_BACKOFF_MIN_DEFAULT,
            max_backoff: RETRY_BACKOFF_MAX_DEFAULT,
            use_retry_after: USE_RETRY_AFTER_DEFAULT,
            follow_redirects: true,
            verify_peer: false,
            verify_host: false,
            dns_cache_timeout: -1,
            headers_only: false,
            last_modified: 0,
        }
    }
}

impl RequestOptions {
    /// `HttpOptions::setHeadersOnly(true)`: also wants headers and stops
    /// verifying the peer.
    pub fn set_headers_only(&mut self, nobody: bool) {
        self.headers_only = nobody;
        if nobody {
            self.want_headers = true;
            self.verify_peer = false;
        }
    }
}

/// `HttpRequest::EPolicyOption`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(i32)]
pub enum PolicyOption {
    ConnectionLimit = 0,
    PerHostConnectionLimit = 1,
    CaPath = 2,
    CaFile = 3,
    HttpProxy = 4,
    LlProxy = 5,
    Trace = 6,
    PipeliningDepth = 7,
    ThrottleRate = 8,
    SslVerifyCallback = 9,
}

impl PolicyOption {
    pub fn from_i32(v: i32) -> Option<PolicyOption> {
        Some(match v {
            0 => PolicyOption::ConnectionLimit,
            1 => PolicyOption::PerHostConnectionLimit,
            2 => PolicyOption::CaPath,
            3 => PolicyOption::CaFile,
            4 => PolicyOption::HttpProxy,
            5 => PolicyOption::LlProxy,
            6 => PolicyOption::Trace,
            7 => PolicyOption::PipeliningDepth,
            8 => PolicyOption::ThrottleRate,
            9 => PolicyOption::SslVerifyCallback,
            _ => return None,
        })
    }

    /// `HttpService::sOptionDesc`: (is long, is dynamic, is global, is class,
    /// is callback).
    pub fn descriptor(self) -> OptionDescriptor {
        let d = |is_long, is_dynamic, is_global, is_class, is_callback| OptionDescriptor {
            is_long,
            is_dynamic,
            is_global,
            is_class,
            is_callback,
        };
        match self {
            PolicyOption::ConnectionLimit => d(true, true, true, true, false),
            PolicyOption::PerHostConnectionLimit => d(true, true, false, true, false),
            PolicyOption::CaPath => d(false, false, true, false, false),
            PolicyOption::CaFile => d(false, false, true, false, false),
            PolicyOption::HttpProxy => d(false, true, true, false, false),
            PolicyOption::LlProxy => d(true, true, true, false, false),
            PolicyOption::Trace => d(true, true, true, false, false),
            PolicyOption::PipeliningDepth => d(true, true, false, true, false),
            PolicyOption::ThrottleRate => d(true, true, false, true, false),
            PolicyOption::SslVerifyCallback => d(false, false, true, false, true),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct OptionDescriptor {
    pub is_long: bool,
    pub is_dynamic: bool,
    pub is_global: bool,
    pub is_class: bool,
    pub is_callback: bool,
}

fn clamp<T: PartialOrd>(v: T, lo: T, hi: T) -> T {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

/// `HttpPolicyClass`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassOptions {
    pub connection_limit: i64,
    pub per_host_connection_limit: i64,
    pub pipelining: i64,
    pub throttle_rate: i64,
}

impl Default for ClassOptions {
    fn default() -> Self {
        ClassOptions {
            connection_limit: CONNECTION_LIMIT_DEFAULT,
            per_host_connection_limit: CONNECTION_LIMIT_DEFAULT,
            pipelining: PIPELINING_DEFAULT,
            throttle_rate: THROTTLE_RATE_DEFAULT,
        }
    }
}

impl ClassOptions {
    pub fn set(&mut self, opt: PolicyOption, value: i64) -> Status {
        match opt {
            PolicyOption::ConnectionLimit => {
                self.connection_limit = clamp(value, CONNECTION_LIMIT_MIN, CONNECTION_LIMIT_MAX)
            }
            PolicyOption::PerHostConnectionLimit => {
                self.per_host_connection_limit =
                    clamp(value, CONNECTION_LIMIT_MIN, self.connection_limit)
            }
            PolicyOption::PipeliningDepth => self.pipelining = clamp(value, 0, PIPELINING_MAX),
            PolicyOption::ThrottleRate => self.throttle_rate = clamp(value, 0, 1_000_000),
            _ => return Status::llcore(he::INVALID_ARG),
        }
        Status::OK
    }

    pub fn get(&self, opt: PolicyOption) -> Result<i64, Status> {
        Ok(match opt {
            PolicyOption::ConnectionLimit => self.connection_limit,
            PolicyOption::PerHostConnectionLimit => self.per_host_connection_limit,
            PolicyOption::PipeliningDepth => self.pipelining,
            PolicyOption::ThrottleRate => self.throttle_rate,
            _ => return Err(Status::llcore(he::INVALID_ARG)),
        })
    }
}

/// `HttpPolicyGlobal`. The SSL verification callback itself lives on the
/// C++ side; the transport only needs to know that one is set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalOptions {
    pub connection_limit: i64,
    pub ca_path: String,
    pub ca_file: String,
    pub http_proxy: String,
    pub trace: i64,
    pub use_llproxy: i64,
    pub ssl_ctx_callback: bool,
}

impl Default for GlobalOptions {
    fn default() -> Self {
        GlobalOptions {
            connection_limit: CONNECTION_LIMIT_DEFAULT,
            ca_path: String::new(),
            ca_file: String::new(),
            http_proxy: String::new(),
            trace: TRACE_OFF as i64,
            use_llproxy: 0,
            ssl_ctx_callback: false,
        }
    }
}

impl GlobalOptions {
    pub fn set_long(&mut self, opt: PolicyOption, value: i64) -> Status {
        match opt {
            PolicyOption::ConnectionLimit => {
                self.connection_limit = clamp(value, CONNECTION_LIMIT_MIN, CONNECTION_LIMIT_MAX)
            }
            PolicyOption::Trace => self.trace = clamp(value, TRACE_MIN as i64, TRACE_MAX as i64),
            PolicyOption::LlProxy => self.use_llproxy = clamp(value, 0, 1),
            _ => return Status::llcore(he::INVALID_ARG),
        }
        Status::OK
    }

    pub fn set_string(&mut self, opt: PolicyOption, value: &str) -> Status {
        match opt {
            PolicyOption::CaPath => self.ca_path = value.to_owned(),
            PolicyOption::CaFile => self.ca_file = value.to_owned(),
            PolicyOption::HttpProxy => self.http_proxy = value.to_owned(),
            _ => return Status::llcore(he::INVALID_ARG),
        }
        Status::OK
    }

    pub fn set_callback(&mut self, opt: PolicyOption, set: bool) -> Status {
        match opt {
            PolicyOption::SslVerifyCallback => self.ssl_ctx_callback = set,
            _ => return Status::llcore(he::INVALID_ARG),
        }
        Status::OK
    }

    pub fn get_long(&self, opt: PolicyOption) -> Result<i64, Status> {
        Ok(match opt {
            PolicyOption::ConnectionLimit => self.connection_limit,
            PolicyOption::Trace => self.trace,
            PolicyOption::LlProxy => self.use_llproxy,
            _ => return Err(Status::llcore(he::INVALID_ARG)),
        })
    }

    pub fn get_string(&self, opt: PolicyOption) -> Result<String, Status> {
        Ok(match opt {
            PolicyOption::CaPath => self.ca_path.clone(),
            PolicyOption::CaFile => self.ca_file.clone(),
            PolicyOption::HttpProxy => self.http_proxy.clone(),
            _ => return Err(Status::llcore(he::INVALID_ARG)),
        })
    }
}

/// The validation `HttpService::{get,set}PolicyOption` applied to every
/// call: option in range and of the asked type, class in range, global or
/// class setting permitted, and for a set while running, dynamic.
pub fn check_option(
    opt: Option<PolicyOption>,
    want_long: bool,
    pclass: u32,
    last_policy: u32,
    setting: bool,
    running: bool,
) -> Result<PolicyOption, Status> {
    let opt = opt.ok_or(Status::llcore(he::INVALID_ARG))?;
    let d = opt.descriptor();
    let global = pclass == GLOBAL_POLICY_ID;
    if d.is_long != want_long
        || (!global && pclass > last_policy)
        || (global && !d.is_global)
        || (!global && !d.is_class)
        || (setting && running && !d.is_dynamic)
    {
        return Err(Status::llcore(he::INVALID_ARG));
    }
    Ok(opt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_clamps() {
        let mut c = ClassOptions::default();
        assert!(c.set(PolicyOption::ConnectionLimit, 1000).is_ok());
        assert_eq!(c.connection_limit, 256);
        assert!(c.set(PolicyOption::ConnectionLimit, 3).is_ok());
        assert!(c.set(PolicyOption::PerHostConnectionLimit, 500).is_ok());
        assert_eq!(c.per_host_connection_limit, 3);
        assert!(c.set(PolicyOption::PipeliningDepth, 99).is_ok());
        assert_eq!(c.pipelining, 20);
        assert_eq!(
            c.set(PolicyOption::Trace, 1),
            Status::llcore(he::INVALID_ARG)
        );
    }

    #[test]
    fn option_checks() {
        assert!(
            check_option(
                Some(PolicyOption::CaFile),
                false,
                GLOBAL_POLICY_ID,
                0,
                true,
                false
            )
            .is_ok()
        );
        assert!(check_option(Some(PolicyOption::CaFile), false, 0, 0, true, false).is_err());
        assert!(
            check_option(
                Some(PolicyOption::CaFile),
                true,
                GLOBAL_POLICY_ID,
                0,
                true,
                false
            )
            .is_err()
        );
        assert!(
            check_option(
                Some(PolicyOption::ThrottleRate),
                true,
                GLOBAL_POLICY_ID,
                0,
                true,
                false
            )
            .is_err()
        );
        assert!(
            check_option(
                Some(PolicyOption::ConnectionLimit),
                true,
                17,
                3,
                true,
                false
            )
            .is_err()
        );
        assert!(check_option(Some(PolicyOption::ConnectionLimit), true, 3, 3, true, true).is_ok());
        assert!(
            check_option(
                Some(PolicyOption::CaFile),
                false,
                GLOBAL_POLICY_ID,
                0,
                true,
                true
            )
            .is_err()
        );
        assert!(check_option(None, true, 0, 0, true, false).is_err());
    }
}
