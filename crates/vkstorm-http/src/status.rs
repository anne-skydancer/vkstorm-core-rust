//! `LLCore::HttpStatus`: a status is a (type, code) pair. The type is one
//! of the three sources (`EXT_CURL_EASY`, `EXT_CURL_MULTI`, `LLCORE`) with
//! the code from that domain, or an HTTP status 100-999 with the code being
//! `HE_SUCCESS` or `HE_REPLY_ERROR`. Success is `code == 0` whatever the
//! type, as in the C++.

use crate::sys;

/// Error codes of the library itself (`LLCore::HttpError`).
pub mod he {
    pub const SUCCESS: i16 = 0;
    pub const REPLY_ERROR: i16 = 1;
    pub const SHUTTING_DOWN: i16 = 2;
    pub const OP_CANCELED: i16 = 3;
    pub const INV_CONTENT_RANGE_HDR: i16 = 4;
    pub const HANDLE_NOT_FOUND: i16 = 5;
    pub const INVALID_ARG: i16 = 6;
    pub const OPT_NOT_SET: i16 = 7;
    pub const OPT_NOT_DYNAMIC: i16 = 8;
    pub const INVALID_HTTP_STATUS: i16 = 9;
    pub const BAD_ALLOC: i16 = 10;
}

/// The status types (`HttpStatus::type_enum_t` values).
pub const EXT_CURL_EASY: u16 = 0;
pub const EXT_CURL_MULTI: u16 = 1;
pub const LLCORE: u16 = 2;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct Status {
    pub kind: u16,
    pub code: i16,
}

impl Default for Status {
    fn default() -> Self {
        Status::OK
    }
}

impl Status {
    pub const OK: Status = Status {
        kind: LLCORE,
        code: he::SUCCESS,
    };

    pub const fn llcore(code: i16) -> Status {
        Status { kind: LLCORE, code }
    }

    pub const fn easy(code: i32) -> Status {
        Status {
            kind: EXT_CURL_EASY,
            code: code as i16,
        }
    }

    pub const fn multi(code: i32) -> Status {
        Status {
            kind: EXT_CURL_MULTI,
            code: code as i16,
        }
    }

    /// An HTTP status: 2xx is success, anything else a reply error.
    pub const fn http(status: i32) -> Status {
        let ok = status >= 200 && status <= 299;
        Status {
            kind: status as u16,
            code: if ok { he::SUCCESS } else { he::REPLY_ERROR },
        }
    }

    pub fn is_ok(self) -> bool {
        self.code == 0
    }

    pub fn is_http(self) -> bool {
        (100..=999).contains(&self.kind)
    }

    /// `HttpStatus::isRetryable`: statuses a retry might clear.
    pub fn is_retryable(self) -> bool {
        (self.is_http() && self.kind >= 499 && self.kind <= 599)
            || self == Status::easy(sys::CURLE_COULDNT_CONNECT)
            || self == Status::easy(sys::CURLE_COULDNT_RESOLVE_PROXY)
            || self == Status::easy(sys::CURLE_COULDNT_RESOLVE_HOST)
            || self == Status::easy(sys::CURLE_SEND_ERROR)
            || self == Status::easy(sys::CURLE_RECV_ERROR)
            || self == Status::easy(sys::CURLE_UPLOAD_FAILED)
            || self == Status::easy(sys::CURLE_OPERATION_TIMEDOUT)
            || self == Status::easy(sys::CURLE_HTTP_POST_ERROR)
            || self == Status::easy(sys::CURLE_PARTIAL_FILE)
            || self == Status::llcore(he::INVALID_HTTP_STATUS)
            || self == Status::llcore(he::INV_CONTENT_RANGE_HDR)
    }

    /// `HttpStatus::toTerseString`: `<Easy|Multi|Core|Http|Unknown>_<value>`.
    pub fn terse(self) -> String {
        let value = self.code as u16 as u32;
        match self.kind {
            EXT_CURL_EASY => format!("Easy_{value}"),
            EXT_CURL_MULTI => format!("Multi_{value}"),
            LLCORE => format!("Core_{value}"),
            _ if self.is_http() => format!("Http_{}", self.kind),
            _ => format!("Unknown_{value}"),
        }
    }

    /// `HttpStatus::toString`: empty for success, else the source's text.
    pub fn message(self) -> String {
        if self.is_ok() {
            return String::new();
        }
        match self.kind {
            EXT_CURL_EASY => sys::easy_strerror(self.code as i32),
            EXT_CURL_MULTI => sys::multi_strerror(self.code as i32),
            LLCORE => llcore_text(self.code).unwrap_or("Unknown error").to_owned(),
            _ if self.is_http() => http_text(self.kind).unwrap_or("Unknown error").to_owned(),
            _ => "Unknown error".to_owned(),
        }
    }

    /// `HttpStatus::operator U32`: the type in the high half, the code low.
    pub fn as_u32(self) -> u32 {
        (self.kind as u32) << 16 | (self.code as u16 as u32)
    }
}

pub fn llcore_text(code: i16) -> Option<&'static str> {
    const TEXT: [&str; 11] = [
        "",
        "HTTP error reply status",
        "Services shutting down",
        "Operation canceled",
        "Invalid Content-Range header encountered",
        "Request handle not found",
        "Invalid datatype for argument or option",
        "Option has not been explicitly set",
        "Option is not dynamic and must be set early",
        "Invalid HTTP status code received from server",
        "Could not allocate required resource",
    ];
    usize::try_from(code)
        .ok()
        .and_then(|i| TEXT.get(i).copied())
}

pub fn http_text(status: u16) -> Option<&'static str> {
    const TEXT: [(u16, &str); 41] = [
        (100, "Continue"),
        (101, "Switching Protocols"),
        (200, "OK"),
        (201, "Created"),
        (202, "Accepted"),
        (203, "Non-Authoritative Information"),
        (204, "No Content"),
        (205, "Reset Content"),
        (206, "Partial Content"),
        (300, "Multiple Choices"),
        (301, "Moved Permanently"),
        (302, "Found"),
        (303, "See Other"),
        (304, "Not Modified"),
        (305, "Use Proxy"),
        (307, "Temporary Redirect"),
        (400, "Bad Request"),
        (401, "Unauthorized"),
        (402, "Payment Required"),
        (403, "Forbidden"),
        (404, "Not Found"),
        (405, "Method Not Allowed"),
        (406, "Not Acceptable"),
        (407, "Proxy Authentication Required"),
        (408, "Request Time-out"),
        (409, "Conflict"),
        (410, "Gone"),
        (411, "Length Required"),
        (412, "Precondition Failed"),
        (413, "Request Entity Too Large"),
        (414, "Request-URI Too Large"),
        (415, "Unsupported Media Type"),
        (416, "Requested range not satisfiable"),
        (417, "Expectation Failed"),
        (499, "Linden Catch-All"),
        (500, "Internal Server Error"),
        (501, "Not Implemented"),
        (502, "Bad Gateway"),
        (503, "Service Unavailable"),
        (504, "Gateway Time-out"),
        (505, "HTTP Version not supported"),
    ];
    TEXT.binary_search_by_key(&status, |e| e.0)
        .ok()
        .map(|i| TEXT[i].1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_success_and_error() {
        assert!(Status::http(200).is_ok());
        assert!(Status::http(206).is_ok());
        assert!(!Status::http(404).is_ok());
        assert!(Status::http(404).is_http());
        assert_eq!(Status::http(503).terse(), "Http_503");
        assert_eq!(Status::http(503).message(), "Service Unavailable");
        assert_eq!(Status::llcore(he::OP_CANCELED).terse(), "Core_3");
        assert_eq!(Status::easy(7).terse(), "Easy_7");
        assert_eq!(Status::easy(-1).terse(), "Easy_65535");
    }

    #[test]
    fn retryable() {
        assert!(Status::http(503).is_retryable());
        assert!(Status::http(499).is_retryable());
        assert!(!Status::http(404).is_retryable());
        assert!(!Status::http(200).is_retryable());
        assert!(Status::easy(sys::CURLE_PARTIAL_FILE).is_retryable());
        assert!(!Status::easy(60).is_retryable());
        assert!(Status::llcore(he::INV_CONTENT_RANGE_HDR).is_retryable());
        assert!(!Status::llcore(he::OP_CANCELED).is_retryable());
    }

    #[test]
    fn as_u32_concatenates() {
        assert_eq!(Status::http(404).as_u32(), 0x0194_0001);
        assert_eq!(Status::easy(28).as_u32(), 28);
    }
}
