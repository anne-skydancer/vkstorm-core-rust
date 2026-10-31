//! What the coroutine adapter makes of an AIS reply: the result the coroutine
//! receives, built from the raw HTTP response (`LLCoreHttpUtil::
//! HttpCoroHandler::onCompleted` with the LLSD handler `HttpCoroLLSDHandler`,
//! `indra/llmessage/llcorehttputil.cpp`), with AIS's depth cap.
//!
//! The viewer's AIS-only adapter variant hands this the response's status,
//! message, URL, headers and body instead of parsing the body into a C++
//! `LLSD` tree; the result is a [`Value`] that [`process_result`] and the
//! parse read directly. The scheduling (a success body over
//! [`WORKER_THRESHOLD`] parses on the `General` work queue) stays with the
//! C++ caller; this is a pure function, safe on any thread.
//!
//! The rules, as the C++ applies them (the viewer's adapter corpus,
//! `ais/adapter`, records each):
//!
//! - an error status (`!status`): an empty map, or for a 4xx the body when it
//!   parses (a map becomes the result, another value goes under `content`);
//! - a success: the body parsed, an empty or absent body as undefined (so
//!   `{content: undefined}`), a value that is not a map under `content`; an
//!   unparsable body is an empty map, and its status becomes 499 "Failed to
//!   deserialize LLSD." only when the content type is exactly
//!   `application/llsd+xml` (the first `content-type` header; a charset or
//!   other case keeps the success status);
//! - then `http_result`: `{success, type, status, message, url, headers}`,
//!   replacing a body's own key; an empty header value reads `true`, a
//!   repeated name keeps the last value; after an error status (the 499
//!   included) also `error_body`, the whole body as a string.
//!
//! An unparsable body is malformed XML or, with the AIS depth cap
//! ([`AIS_MAX_REPLY_DEPTH`], an approved fix of 2026-10-29), a value nested
//! that many containers deep.
//!
//! [`process_result`]: super::process_result

use vkstorm_llsd::{Format, Limits, PARSE_FAILURE, Value};

use super::result::{HTTP_RESULTS, HttpStatus};
use crate::llsd::coerce::insert;

/// `llaisapi.cpp` `AIS_MAX_REPLY_DEPTH`: the LLSD depth limit AIS sets on its
/// adapter (`LLSDParser`'s `max_depth`): a value inside this many containers
/// makes the body unparsable.
pub const AIS_MAX_REPLY_DEPTH: i32 = 256;
/// `HttpCoroHandler::onCompleted`'s `MAX_BODY_SIZE_THRESHOLD`: a success body
/// larger than this parses on the `General` work queue.
pub const WORKER_THRESHOLD: usize = 65536;
/// `HTTP_IN_HEADER_CONTENT_TYPE`.
pub const CONTENT_TYPE: &[u8] = b"content-type";
/// `HTTP_CONTENT_LLSD_XML`.
pub const LLSD_XML: &[u8] = b"application/llsd+xml";
/// The 499's message.
pub const PARSE_FAILED_MESSAGE: &[u8] = b"Failed to deserialize LLSD.";
/// `HttpStatus::LLCORE`, the type of the library's own errors.
pub const STATUS_LLCORE: u16 = 2;
/// `HE_HANDLE_NOT_FOUND`: a reply to a request already canceled.
pub const HE_HANDLE_NOT_FOUND: i16 = 5;

/// An HTTP response as the adapter's handler sees it.
#[derive(Clone, Copy, Debug)]
pub struct Response<'a> {
    pub status: HttpStatus,
    /// `HttpStatus::getMessage()`.
    pub message: &'a [u8],
    /// `getRequestURL()`.
    pub url: &'a [u8],
    /// `getHeaders()`, in order (names as `HttpHeaders::appendNormal` left
    /// them: lower case).
    pub headers: &'a [(&'a [u8], &'a [u8])],
    /// `getBody()`: `None` when the response has no body buffer.
    pub body: Option<&'a [u8]>,
    /// The parse's depth limit, [`AIS_MAX_REPLY_DEPTH`] for AIS (-1 is
    /// unlimited).
    pub max_depth: i32,
}

/// The result the coroutine receives, and the status it carries.
#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    /// The result map, `http_result` included.
    pub value: Value,
    /// The status after the handler (499 for an unparsable `application/
    /// llsd+xml` success).
    pub status: HttpStatus,
    /// Its message.
    pub message: Vec<u8>,
}

/// Whether the success body is large enough for the `General` work queue.
pub fn parses_on_worker(r: &Response<'_>) -> bool {
    r.status.is_ok() && r.body.map_or(0, <[u8]>::len) > WORKER_THRESHOLD
}

/// `HttpCoroLLSDHandler::parseBody`: success and the value. An empty or
/// absent body is a success with an undefined value; an XML parse failure
/// (malformed, or a value `max_depth` containers deep) is a failure.
fn parse_body(r: &Response<'_>) -> (bool, Value) {
    let bytes = r.body.unwrap_or(&[]);
    if bytes.is_empty() {
        return (true, Value::Undefined);
    }
    let limits = Limits {
        max_bytes: None,
        max_depth: r.max_depth,
    };
    let (n, v) = vkstorm_llsd::parse(Format::Xml, bytes, limits);
    if n == PARSE_FAILURE {
        (false, Value::Undefined)
    } else {
        (true, v.unwrap_or(Value::Undefined))
    }
}

/// The first header of `name` (`HttpHeaders::find`).
fn find_header<'a>(r: &Response<'a>, name: &[u8]) -> Option<&'a [u8]> {
    r.headers.iter().find(|(n, _)| *n == name).map(|(_, v)| *v)
}

/// A value under `content` in a new map.
fn content_map(v: Value) -> Value {
    let mut m = Value::map();
    insert(&mut m, "content", v);
    m
}

/// `HttpCoroHandler::onCompleted` with the LLSD handler: the result posted to
/// the coroutine, or `None` when nothing is posted (`HE_HANDLE_NOT_FOUND`: a
/// reply to a request already canceled, which the handler ignores).
pub fn adapter_result(r: &Response<'_>) -> Option<Reply> {
    if r.status.kind == STATUS_LLCORE && r.status.status == HE_HANDLE_NOT_FOUND {
        return None;
    }
    let mut status = r.status;
    let mut message = r.message.to_vec();
    let mut result;
    if !status.is_ok() {
        result = Value::map();
        if (400..500).contains(&status.kind) {
            let (_, body) = parse_body(r);
            match body {
                Value::Undefined => {}
                Value::Map(_) => result = body,
                other => result = content_map(other),
            }
        }
    } else {
        let (success, body) = parse_body(r);
        if !success {
            if find_header(r, CONTENT_TYPE) == Some(LLSD_XML) {
                status = HttpStatus::http(499);
                message = PARSE_FAILED_MESSAGE.to_vec();
            }
            result = Value::map();
        } else if matches!(body, Value::Map(_)) {
            result = body;
        } else {
            result = content_map(body);
        }
    }

    // replyPost: buildStatusEntry, then the error body.
    let mut headers = Value::map();
    for (name, value) in r.headers {
        let v = if value.is_empty() {
            Value::Boolean(true)
        } else {
            Value::String(value.to_vec())
        };
        if let Value::Map(m) = &mut headers {
            m.insert(name.to_vec(), v);
        }
    }
    let mut http = Value::map();
    insert(&mut http, "success", Value::Boolean(status.is_ok()));
    insert(&mut http, "type", Value::Integer(i32::from(status.kind)));
    insert(
        &mut http,
        "status",
        Value::Integer(i32::from(status.status)),
    );
    insert(&mut http, "message", Value::String(message.clone()));
    insert(&mut http, "url", Value::String(r.url.to_vec()));
    insert(&mut http, "headers", headers);
    if !status.is_ok() {
        insert(
            &mut http,
            "error_body",
            Value::String(r.body.unwrap_or(&[]).to_vec()),
        );
    }
    insert(&mut result, HTTP_RESULTS, http);
    Some(Reply {
        value: result,
        status,
        message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llsd::coerce::get;

    fn response<'a>(
        code: u16,
        headers: &'a [(&'a [u8], &'a [u8])],
        body: Option<&'a [u8]>,
    ) -> Response<'a> {
        Response {
            status: HttpStatus::http(code),
            message: b"",
            url: b"https://ais.example/x",
            headers,
            body,
            max_depth: AIS_MAX_REPLY_DEPTH,
        }
    }

    fn nested(levels: usize) -> Vec<u8> {
        let mut s = b"<llsd>".to_vec();
        for _ in 0..levels {
            s.extend_from_slice(b"<array>");
        }
        s.extend_from_slice(b"<integer>1</integer>");
        for _ in 0..levels {
            s.extend_from_slice(b"</array>");
        }
        s.extend_from_slice(b"</llsd>");
        s
    }

    const XML: &[(&[u8], &[u8])] = &[(b"content-type", b"application/llsd+xml")];
    const XML_CHARSET: &[(&[u8], &[u8])] =
        &[(b"content-type", b"application/llsd+xml; charset=utf-8")];

    #[test]
    fn unparsable_success_is_499_only_for_the_exact_content_type() {
        let bad: &[u8] = b"<llsd><map>";
        let r = adapter_result(&response(200, XML, Some(bad))).unwrap();
        assert_eq!(r.status, HttpStatus::http(499));
        let http = get(&r.value, HTTP_RESULTS);
        assert_eq!(get(http, "error_body"), &Value::String(bad.to_vec()));
        let r = adapter_result(&response(200, XML_CHARSET, Some(bad))).unwrap();
        assert_eq!(r.status, HttpStatus::http(200));
        assert!(!crate::llsd::coerce::has(
            get(&r.value, HTTP_RESULTS),
            "error_body"
        ));
    }

    #[test]
    fn depth_cap() {
        let ok = nested(AIS_MAX_REPLY_DEPTH as usize - 1);
        let deep = nested(AIS_MAX_REPLY_DEPTH as usize);
        assert!(
            adapter_result(&response(200, XML, Some(&ok)))
                .unwrap()
                .status
                .is_ok()
        );
        let r = adapter_result(&response(200, XML, Some(&deep))).unwrap();
        assert_eq!(r.status, HttpStatus::http(499));
        let hostile = nested(1_000_000);
        let r = adapter_result(&response(200, XML, Some(&hostile))).unwrap();
        assert_eq!(r.status, HttpStatus::http(499));
    }

    #[test]
    fn shapes() {
        // An absent body on success: {content: undefined}.
        let r = adapter_result(&response(200, &[], None)).unwrap();
        assert_eq!(get(&r.value, "content"), &Value::Undefined);
        assert!(crate::llsd::coerce::has(&r.value, "content"));
        // A 5xx body is not parsed; a 4xx array goes under content.
        let arr: &[u8] = b"<llsd><array><integer>1</integer></array></llsd>";
        let r = adapter_result(&response(500, XML, Some(arr))).unwrap();
        assert!(!crate::llsd::coerce::has(&r.value, "content"));
        let r = adapter_result(&response(404, XML, Some(arr))).unwrap();
        assert!(matches!(get(&r.value, "content"), Value::Array(_)));
        // The canceled request's late reply is not posted.
        let mut gone = response(200, &[], None);
        gone.status = HttpStatus {
            kind: STATUS_LLCORE,
            status: HE_HANDLE_NOT_FOUND,
        };
        assert!(adapter_result(&gone).is_none());
        // Worker threshold: success bodies over 64 KB.
        let big = vec![b' '; WORKER_THRESHOLD + 1];
        assert!(parses_on_worker(&response(200, &[], Some(&big))));
        assert!(!parses_on_worker(&response(404, &[], Some(&big))));
        assert!(!parses_on_worker(&response(
            200,
            &[],
            Some(&big[..WORKER_THRESHOLD])
        )));
    }
}
