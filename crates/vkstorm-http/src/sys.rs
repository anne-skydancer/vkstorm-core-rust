//! The libcurl binding layer: the handful of libcurl functions, options and
//! codes the transport uses, declared by hand against the stable libcurl ABI
//! (option numbers and result codes have been fixed since libcurl 7.x; the
//! viewer ships 7.54.1 and the constants here exist in that version).
//!
//! No `curl-sys`: its build script either finds a system libcurl (vcpkg on
//! Windows, pkg-config elsewhere) or builds its bundled copy with the
//! platform TLS stack, and neither is the libcurl the viewer stages
//! (OpenSSL, nghttp2, zlib-ng, prebuilt by autobuild). The symbols declared
//! here resolve at the viewer's final link against that library; the crate's
//! own tests link one through the `link-curl` feature (see build.rs).
//!
//! Everything `unsafe` in the crate is here and in [`crate::transport`].

#![allow(non_camel_case_types, dead_code)]

use std::ffi::{c_char, c_int, c_long, c_void, CStr};

pub type CURL = c_void;
pub type CURLM = c_void;
pub type CURLcode = c_int;
pub type CURLMcode = c_int;
pub type CURLoption = c_int;
pub type CURLMoption = c_int;
pub type CURLINFO = c_int;
pub type curl_infotype = c_int;
pub type curl_off_t = i64;

#[repr(C)]
pub struct curl_slist {
    pub data: *mut c_char,
    pub next: *mut curl_slist,
}

#[repr(C)]
pub union CURLMsgData {
    pub whatever: *mut c_void,
    pub result: CURLcode,
}

#[repr(C)]
pub struct CURLMsg {
    pub msg: c_int,
    pub easy_handle: *mut CURL,
    pub data: CURLMsgData,
}

pub const CURLMSG_DONE: c_int = 1;

pub const CURL_GLOBAL_SSL: c_long = 1 << 0;
pub const CURL_GLOBAL_WIN32: c_long = 1 << 1;
pub const CURL_GLOBAL_ALL: c_long = CURL_GLOBAL_SSL | CURL_GLOBAL_WIN32;

const LONG: CURLoption = 0;
const OBJECTPOINT: CURLoption = 10000;
const FUNCTIONPOINT: CURLoption = 20000;
const OFF_T: CURLoption = 30000;

pub const CURLOPT_WRITEDATA: CURLoption = OBJECTPOINT + 1;
pub const CURLOPT_URL: CURLoption = OBJECTPOINT + 2;
pub const CURLOPT_PROXY: CURLoption = OBJECTPOINT + 4;
pub const CURLOPT_PROXYUSERPWD: CURLoption = OBJECTPOINT + 6;
pub const CURLOPT_READDATA: CURLoption = OBJECTPOINT + 9;
pub const CURLOPT_WRITEFUNCTION: CURLoption = FUNCTIONPOINT + 11;
pub const CURLOPT_READFUNCTION: CURLoption = FUNCTIONPOINT + 12;
pub const CURLOPT_TIMEOUT: CURLoption = LONG + 13;
pub const CURLOPT_INFILESIZE: CURLoption = LONG + 14;
pub const CURLOPT_POSTFIELDS: CURLoption = OBJECTPOINT + 15;
pub const CURLOPT_USERAGENT: CURLoption = OBJECTPOINT + 18;
pub const CURLOPT_HTTPHEADER: CURLoption = OBJECTPOINT + 23;
pub const CURLOPT_HEADERDATA: CURLoption = OBJECTPOINT + 29;
pub const CURLOPT_COOKIEFILE: CURLoption = OBJECTPOINT + 31;
pub const CURLOPT_TIMECONDITION: CURLoption = LONG + 33;
pub const CURLOPT_TIMEVALUE: CURLoption = LONG + 34;
pub const CURLOPT_CUSTOMREQUEST: CURLoption = OBJECTPOINT + 36;
pub const CURLOPT_VERBOSE: CURLoption = LONG + 41;
pub const CURLOPT_NOPROGRESS: CURLoption = LONG + 43;
pub const CURLOPT_NOBODY: CURLoption = LONG + 44;
pub const CURLOPT_UPLOAD: CURLoption = LONG + 46;
pub const CURLOPT_POST: CURLoption = LONG + 47;
pub const CURLOPT_FOLLOWLOCATION: CURLoption = LONG + 52;
pub const CURLOPT_AUTOREFERER: CURLoption = LONG + 58;
pub const CURLOPT_PROXYPORT: CURLoption = LONG + 59;
pub const CURLOPT_POSTFIELDSIZE: CURLoption = LONG + 60;
pub const CURLOPT_SSL_VERIFYPEER: CURLoption = LONG + 64;
pub const CURLOPT_CAINFO: CURLoption = OBJECTPOINT + 65;
pub const CURLOPT_MAXREDIRS: CURLoption = LONG + 68;
pub const CURLOPT_CONNECTTIMEOUT: CURLoption = LONG + 78;
pub const CURLOPT_HEADERFUNCTION: CURLoption = FUNCTIONPOINT + 79;
pub const CURLOPT_HTTPGET: CURLoption = LONG + 80;
pub const CURLOPT_SSL_VERIFYHOST: CURLoption = LONG + 81;
pub const CURLOPT_HTTP_VERSION: CURLoption = LONG + 84;
pub const CURLOPT_DNS_CACHE_TIMEOUT: CURLoption = LONG + 92;
pub const CURLOPT_DEBUGFUNCTION: CURLoption = FUNCTIONPOINT + 94;
pub const CURLOPT_DEBUGDATA: CURLoption = OBJECTPOINT + 95;
pub const CURLOPT_CAPATH: CURLoption = OBJECTPOINT + 97;
pub const CURLOPT_NOSIGNAL: CURLoption = LONG + 99;
pub const CURLOPT_PROXYTYPE: CURLoption = LONG + 101;
/// `CURLOPT_ENCODING` in the viewer's libcurl, where it still carries that
/// name; the same option number.
pub const CURLOPT_ACCEPT_ENCODING: CURLoption = OBJECTPOINT + 102;
pub const CURLOPT_PRIVATE: CURLoption = OBJECTPOINT + 103;
pub const CURLOPT_SSL_CTX_FUNCTION: CURLoption = FUNCTIONPOINT + 108;
pub const CURLOPT_SSL_CTX_DATA: CURLoption = OBJECTPOINT + 109;
pub const CURLOPT_IPRESOLVE: CURLoption = LONG + 113;
pub const CURLOPT_SEEKFUNCTION: CURLoption = FUNCTIONPOINT + 167;
pub const CURLOPT_SEEKDATA: CURLoption = OBJECTPOINT + 168;
/// Added in 7.59.0; the viewer's 7.54.1 does not have it, so the transport
/// uses `CURLOPT_TIMEVALUE` (a `long`), as the C++ did for that version.
pub const CURLOPT_TIMEVALUE_LARGE: CURLoption = OFF_T + 270;

pub const CURLMOPT_PIPELINING: CURLMoption = LONG + 3;
pub const CURLMOPT_MAX_HOST_CONNECTIONS: CURLMoption = LONG + 7;
pub const CURLMOPT_MAX_PIPELINE_LENGTH: CURLMoption = LONG + 8;
pub const CURLMOPT_MAX_TOTAL_CONNECTIONS: CURLMoption = LONG + 13;

const CURLINFO_STRING: CURLINFO = 0x100000;
const CURLINFO_LONG: CURLINFO = 0x200000;
const CURLINFO_DOUBLE: CURLINFO = 0x300000;
pub const CURLINFO_RESPONSE_CODE: CURLINFO = CURLINFO_LONG + 2;
pub const CURLINFO_TOTAL_TIME: CURLINFO = CURLINFO_DOUBLE + 3;
pub const CURLINFO_SIZE_DOWNLOAD: CURLINFO = CURLINFO_DOUBLE + 8;
pub const CURLINFO_SPEED_DOWNLOAD: CURLINFO = CURLINFO_DOUBLE + 9;
pub const CURLINFO_CONTENT_TYPE: CURLINFO = CURLINFO_STRING + 18;
pub const CURLINFO_PRIVATE: CURLINFO = CURLINFO_STRING + 21;

pub const CURL_IPRESOLVE_V4: c_long = 1;
pub const CURL_HTTP_VERSION_2_0: c_long = 3;
pub const CURL_TIMECOND_IFMODSINCE: c_long = 1;
pub const CURLPROXY_HTTP: c_long = 0;
pub const CURLPROXY_SOCKS5: c_long = 5;

pub const CURLINFO_TEXT: curl_infotype = 0;
pub const CURLINFO_HEADER_IN: curl_infotype = 1;
pub const CURLINFO_HEADER_OUT: curl_infotype = 2;
pub const CURLINFO_DATA_IN: curl_infotype = 3;
pub const CURLINFO_DATA_OUT: curl_infotype = 4;

pub const CURL_SEEKFUNC_OK: c_int = 0;
pub const CURL_SEEKFUNC_FAIL: c_int = 1;
pub const CURL_SEEKFUNC_CANTSEEK: c_int = 2;

pub const CURLE_OK: CURLcode = 0;
pub const CURLE_COULDNT_RESOLVE_PROXY: CURLcode = 5;
pub const CURLE_COULDNT_RESOLVE_HOST: CURLcode = 6;
pub const CURLE_COULDNT_CONNECT: CURLcode = 7;
pub const CURLE_PARTIAL_FILE: CURLcode = 18;
pub const CURLE_UPLOAD_FAILED: CURLcode = 25;
pub const CURLE_OPERATION_TIMEDOUT: CURLcode = 28;
pub const CURLE_HTTP_POST_ERROR: CURLcode = 34;
pub const CURLE_SEND_ERROR: CURLcode = 55;
pub const CURLE_RECV_ERROR: CURLcode = 56;

pub const CURLM_CALL_MULTI_PERFORM: CURLMcode = -1;
pub const CURLM_OK: CURLMcode = 0;

pub type WriteFn = unsafe extern "C" fn(*mut c_char, usize, usize, *mut c_void) -> usize;
pub type ReadFn = unsafe extern "C" fn(*mut c_char, usize, usize, *mut c_void) -> usize;
pub type SeekFn = unsafe extern "C" fn(*mut c_void, curl_off_t, c_int) -> c_int;
pub type SslCtxFn = unsafe extern "C" fn(*mut CURL, *mut c_void, *mut c_void) -> CURLcode;
pub type DebugFn = unsafe extern "C" fn(*mut CURL, curl_infotype, *mut c_char, usize, *mut c_void) -> c_int;

unsafe extern "C" {
    pub fn curl_global_init(flags: c_long) -> CURLcode;
    pub fn curl_version() -> *const c_char;
    pub fn curl_easy_init() -> *mut CURL;
    pub fn curl_easy_duphandle(handle: *mut CURL) -> *mut CURL;
    pub fn curl_easy_reset(handle: *mut CURL);
    pub fn curl_easy_cleanup(handle: *mut CURL);
    pub fn curl_easy_setopt(handle: *mut CURL, option: CURLoption, ...) -> CURLcode;
    pub fn curl_easy_getinfo(handle: *mut CURL, info: CURLINFO, ...) -> CURLcode;
    pub fn curl_easy_strerror(code: CURLcode) -> *const c_char;
    pub fn curl_multi_init() -> *mut CURLM;
    pub fn curl_multi_cleanup(multi: *mut CURLM) -> CURLMcode;
    pub fn curl_multi_add_handle(multi: *mut CURLM, handle: *mut CURL) -> CURLMcode;
    pub fn curl_multi_remove_handle(multi: *mut CURLM, handle: *mut CURL) -> CURLMcode;
    pub fn curl_multi_perform(multi: *mut CURLM, running: *mut c_int) -> CURLMcode;
    pub fn curl_multi_info_read(multi: *mut CURLM, msgs_in_queue: *mut c_int) -> *mut CURLMsg;
    pub fn curl_multi_setopt(multi: *mut CURLM, option: CURLMoption, ...) -> CURLMcode;
    pub fn curl_multi_strerror(code: CURLMcode) -> *const c_char;
    pub fn curl_slist_append(list: *mut curl_slist, string: *const c_char) -> *mut curl_slist;
    pub fn curl_slist_free_all(list: *mut curl_slist);
}

/// `curl_global_init(CURL_GLOBAL_ALL)`, once per process. The viewer calls
/// it itself (`LLCore::LLHttp::initialize`); the crate's tests use this.
pub fn global_init() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        curl_global_init(CURL_GLOBAL_ALL);
    });
}

/// The text of a libcurl easy result code.
pub fn easy_strerror(code: CURLcode) -> String {
    unsafe { CStr::from_ptr(curl_easy_strerror(code)) }.to_string_lossy().into_owned()
}

/// The text of a libcurl multi result code.
pub fn multi_strerror(code: CURLMcode) -> String {
    unsafe { CStr::from_ptr(curl_multi_strerror(code)) }.to_string_lossy().into_owned()
}

/// `curl_version()`.
pub fn version() -> String {
    unsafe { CStr::from_ptr(curl_version()) }.to_string_lossy().into_owned()
}

/// A raw pointer that may cross threads: a libcurl handle or list owned by
/// the service thread, or caller data the crate never dereferences.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SendPtr(pub *mut c_void);

// SAFETY: the pointee is either owned by the service thread (libcurl
// handles and header lists, created and freed there) or opaque caller data
// the crate only hands back; the crate itself never dereferences it.
unsafe impl Send for SendPtr {}
unsafe impl Sync for SendPtr {}

impl SendPtr {
    pub const fn null() -> SendPtr {
        SendPtr(std::ptr::null_mut())
    }

    pub fn is_null(self) -> bool {
        self.0.is_null()
    }
}

impl Default for SendPtr {
    fn default() -> Self {
        SendPtr::null()
    }
}

/// Caller data attached to an operation and handed back with its reply:
/// the viewer stores the `HttpHandler` there. Opaque to the crate.
pub type UserPtr = SendPtr;

/// What the SSL context hook receives for a request whose connection is
/// being set up: the identity of the request and, written back by the
/// hook's verification callback, the status it decided. Lives in the
/// request record, whose address is stable while the request is active.
#[repr(C)]
#[derive(Debug)]
pub struct SslInfo {
    pub handle: u64,
    pub user: UserPtr,
    pub url: *const c_char,
    pub verify_peer: i32,
    pub result_type: u16,
    pub result_code: i16,
}

// SAFETY: the url pointer names the request's own NUL-terminated copy of
// the URL, which lives as long as the record; the hook runs on the service
// thread that owns the record.
unsafe impl Send for SslInfo {}
unsafe impl Sync for SslInfo {}
