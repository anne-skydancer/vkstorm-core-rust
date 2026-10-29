//! The service against the loopback mock server: the scenarios of the
//! viewer's `INTEGRATION_TEST_llcorehttp_parity`, at the crate level.

mod mock;

use std::sync::Arc;
use std::time::Duration;

use mock::{LLSD_OK, MockServer, pattern};
use vkstorm_http::{
    Client, DEFAULT_POLICY_ID, GLOBAL_POLICY_ID, Method, NoHooks, PolicyOption, ReplyKind, Request,
    RequestOptions, Service, Status, TEST_USER_AGENT, he, sys,
};

fn service() -> Service {
    sys::global_init();
    Service::new(Arc::new(NoHooks), TEST_USER_AGENT)
}

fn started() -> (Service, MockServer) {
    let mut s = service();
    s.start_thread();
    (s, MockServer::start())
}

fn options(retries: u32) -> RequestOptions {
    RequestOptions {
        retries,
        ..RequestOptions::default()
    }
}

const WAIT: Duration = Duration::from_secs(60);

fn stop(mut client: Client<'_>, s: &Service) {
    client.stop_thread().unwrap();
    let start = std::time::Instant::now();
    while !s.is_stopped() && start.elapsed() < Duration::from_secs(10) {
        client.update();
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(s.is_stopped());
}

#[test]
fn get_ok_with_headers() {
    let (s, m) = started();
    let mut c = Client::new(&s);
    let opts = RequestOptions {
        want_headers: true,
        ..options(5)
    };
    let h = c
        .get(DEFAULT_POLICY_ID, &m.url("/ok/"), Some(opts))
        .unwrap();
    let r = c.wait_for(1, WAIT).pop().unwrap();
    assert_eq!(r.handle, h);
    assert_eq!(r.status, Status::http(200));
    assert_eq!(r.body, LLSD_OK);
    assert_eq!(r.content_type, "application/llsd+xml");
    assert_eq!(r.method, "GET");
    assert!(r.url.ends_with("/ok/"));
    let headers = r.headers.expect("headers wanted");
    assert!(
        headers
            .iter()
            .any(|(n, v)| n == "content-type" && v == "application/llsd+xml")
    );
    assert!(
        headers
            .iter()
            .all(|(n, _)| n == n.to_ascii_lowercase().as_str())
    );
    assert!(r.stats.size_download > 0.0);
    let h2 = c.get(DEFAULT_POLICY_ID, &m.url("/ok/"), None).unwrap();
    let r2 = c.wait_for(1, WAIT).pop().unwrap();
    assert_eq!(r2.handle, h2);
    assert!(r2.headers.is_none());
    assert_ne!(h, h2, "handles are unique for the service's life");
    let rec = m.records();
    assert_eq!(rec.len(), 2);
    assert_eq!(rec[0].header("User-Agent"), Some(TEST_USER_AGENT));
    assert_eq!(rec[0].header("Connection"), Some("keep-alive"));
    assert_eq!(rec[0].header("Keep-alive"), Some("300"));
    assert_eq!(rec[0].header("Accept-Encoding"), Some("deflate, gzip"));
    assert!(rec[0].header("Pragma").is_none());
    assert_eq!(rec[1].conn, rec[0].conn, "keep-alive reuses the connection");
    let n = c.noop().unwrap();
    let r = c.wait_for(1, WAIT).pop().unwrap();
    assert_eq!(
        (r.handle, r.kind, r.status),
        (n, ReplyKind::NoOp, Status::OK)
    );
    stop(c, &s);
}

#[test]
fn methods_and_bodies() {
    let (s, m) = started();
    let mut c = Client::new(&s);
    let body = b"<llsd><string>parity</string></llsd>".to_vec();
    for (method, wants_body) in [
        (Method::Post, true),
        (Method::Put, true),
        (Method::Patch, true),
        (Method::Delete, false),
        (Method::Copy, false),
        (Method::Move, false),
    ] {
        let mut req = Request::new(DEFAULT_POLICY_ID, method, m.url("/echo/"))
            .header("Content-Type", "application/llsd+xml");
        if wants_body {
            req = req.body(body.clone());
        }
        c.request(req).unwrap();
        let r = c.wait_for(1, WAIT).pop().unwrap();
        assert_eq!(r.status, Status::http(200), "{method:?}");
        assert_eq!(r.method, method.as_str());
        if wants_body {
            assert_eq!(r.body, body);
        }
    }
    let rec = m.records();
    assert_eq!(
        rec.iter().map(|r| r.method.as_str()).collect::<Vec<_>>(),
        ["POST", "PUT", "PATCH", "DELETE", "COPY", "MOVE"]
    );
    assert_eq!(rec[0].header("Content-Length"), Some("36"));
    assert!(rec[0].header("Expect").is_none());
    assert_eq!(rec[1].header("Content-Length"), Some("36"));
    // HEAD
    let opts = RequestOptions {
        headers_only: true,
        want_headers: true,
        ..options(5)
    };
    c.get(DEFAULT_POLICY_ID, &m.url("/ok/"), Some(opts))
        .unwrap();
    let r = c.wait_for(1, WAIT).pop().unwrap();
    assert_eq!(r.status, Status::http(200));
    assert!(r.body.is_empty());
    assert_eq!(m.records().last().unwrap().method, "HEAD");
    // A large body, in both directions.
    let big: Vec<u8> = (0..300_000u32)
        .map(|i| b'a' + ((i * 31) % 26) as u8)
        .collect();
    c.post(DEFAULT_POLICY_ID, &m.url("/echo/"), big.clone(), None)
        .unwrap();
    let r = c.wait_for(1, WAIT).pop().unwrap();
    assert_eq!(r.status, Status::http(200));
    assert_eq!(r.body, big);
    stop(c, &s);
}

#[test]
fn byte_ranges() {
    let (s, m) = started();
    let mut c = Client::new(&s);
    let full = pattern(1000);
    c.get_range(DEFAULT_POLICY_ID, &m.url("/large/1000"), 100, 200, None)
        .unwrap();
    let r = c.wait_for(1, WAIT).pop().unwrap();
    assert_eq!(r.status, Status::http(206));
    assert_eq!(r.range, Some((100, 200, 1000)));
    assert_eq!(r.body, full[100..300]);
    assert_eq!(m.records()[0].header("Range"), Some("bytes=100-299"));
    c.get_range(DEFAULT_POLICY_ID, &m.url("/large/1000"), 500, 0, None)
        .unwrap();
    let r = c.wait_for(1, WAIT).pop().unwrap();
    assert_eq!(r.range, Some((500, 500, 1000)));
    assert_eq!(m.records()[1].header("Range"), Some("bytes=500-"));
    c.get_range(
        DEFAULT_POLICY_ID,
        &m.url("/large/1000"),
        2000,
        10,
        Some(options(2)),
    )
    .unwrap();
    let r = c.wait_for(1, WAIT).pop().unwrap();
    assert_eq!(r.status, Status::http(416));
    assert_eq!(r.retries, 0);
    assert_eq!(r.range, None);
    // One mebibyte, whole and in the middle.
    let mib = pattern(1_048_576);
    c.get_range(
        DEFAULT_POLICY_ID,
        &m.url("/large/1048576"),
        0,
        1_048_576,
        None,
    )
    .unwrap();
    let r = c.wait_for(1, WAIT).pop().unwrap();
    assert_eq!(r.status, Status::http(206));
    assert_eq!(r.range, Some((0, 1_048_576, 1_048_576)));
    assert_eq!(r.body, mib);
    c.get_range(
        DEFAULT_POLICY_ID,
        &m.url("/large/1048576"),
        262_144,
        524_288,
        None,
    )
    .unwrap();
    let r = c.wait_for(1, WAIT).pop().unwrap();
    assert_eq!(r.range, Some((262_144, 524_288, 1_048_576)));
    assert_eq!(r.body, mib[262_144..786_432]);
    stop(c, &s);
}

#[test]
fn errors_4xx_are_not_retried() {
    let (s, m) = started();
    let mut c = Client::new(&s);
    for code in [404, 403, 400] {
        c.get(DEFAULT_POLICY_ID, &m.url(&format!("/status/{code}")), None)
            .unwrap();
        let r = c.wait_for(1, WAIT).pop().unwrap();
        assert_eq!(r.status, Status::http(code));
        assert!(!r.status.is_ok());
        assert_eq!(r.retries, 0);
    }
    c.get(DEFAULT_POLICY_ID, &m.url("/status/204"), None)
        .unwrap();
    let r = c.wait_for(1, WAIT).pop().unwrap();
    assert_eq!(r.status, Status::http(204));
    assert!(r.body.is_empty());
    assert_eq!(m.records().len(), 4);
    stop(c, &s);
}

#[test]
fn retries_5xx_with_backoff() {
    let (s, m) = started();
    let mut c = Client::new(&s);
    let opts = RequestOptions {
        retries: 2,
        min_backoff: 200_000,
        max_backoff: 5_000_000,
        ..RequestOptions::default()
    };
    c.get(DEFAULT_POLICY_ID, &m.url("/status/500"), Some(opts))
        .unwrap();
    let r = c.wait_for(1, WAIT).pop().unwrap();
    assert_eq!(r.status, Status::http(500));
    assert_eq!((r.retries, r.retries_503), (2, 0));
    let rec = m.records();
    assert_eq!(rec.len(), 3);
    let gap1 = rec[1].t_ms - rec[0].t_ms;
    let gap2 = rec[2].t_ms - rec[1].t_ms;
    assert!(
        (150.0..600.0).contains(&gap1),
        "first retry after {gap1} ms"
    );
    assert!(
        (350.0..900.0).contains(&gap2),
        "second retry after {gap2} ms"
    );
    stop(c, &s);
}

#[test]
fn recovers_from_503_with_and_without_retry_after() {
    let (s, m) = started();
    let mut c = Client::new(&s);
    let fast = RequestOptions {
        min_backoff: 200_000,
        max_backoff: 500_000,
        ..RequestOptions::default()
    };
    c.get(DEFAULT_POLICY_ID, &m.url("/flaky/a/2"), Some(fast.clone()))
        .unwrap();
    let r = c.wait_for(1, WAIT).pop().unwrap();
    assert_eq!(r.status, Status::http(200));
    assert_eq!((r.retries, r.retries_503), (2, 2));
    assert_eq!(r.body, LLSD_OK);
    // Retry-After honoured (1 s instead of the 200 ms backoff).
    m.clear();
    c.get(
        DEFAULT_POLICY_ID,
        &m.url("/flaky/b/1/retry/1"),
        Some(fast.clone()),
    )
    .unwrap();
    let r = c.wait_for(1, WAIT).pop().unwrap();
    assert_eq!(r.status, Status::http(200));
    assert_eq!((r.retries, r.retries_503), (1, 1));
    let rec = m.records();
    let gap = rec[1].t_ms - rec[0].t_ms;
    assert!((900.0..1500.0).contains(&gap), "Retry-After gap {gap} ms");
    // Retry-After ignored when asked, and the retry still succeeds: the
    // status of the failed attempt does not stick (the C++ kept it when
    // no header callback was installed, so this request could never
    // recover).
    m.clear();
    let no_ra = RequestOptions {
        use_retry_after: false,
        ..fast.clone()
    };
    c.get(DEFAULT_POLICY_ID, &m.url("/flaky/c/1/retry/1"), Some(no_ra))
        .unwrap();
    let r = c.wait_for(1, WAIT).pop().unwrap();
    assert_eq!(r.status, Status::http(200));
    assert_eq!((r.retries, r.retries_503), (1, 1));
    let rec = m.records();
    let gap = rec[1].t_ms - rec[0].t_ms;
    assert!((150.0..600.0).contains(&gap), "backoff gap {gap} ms");
    // No options at all: default retries, and recovery.
    m.clear();
    c.get(DEFAULT_POLICY_ID, &m.url("/flaky/d/1"), None)
        .unwrap();
    let r = c.wait_for(1, WAIT).pop().unwrap();
    assert_eq!(r.status, Status::http(200));
    assert_eq!(r.retries, 1);
    stop(c, &s);
}

#[test]
fn timeout_is_reported_and_retried() {
    let (s, m) = started();
    let mut c = Client::new(&s);
    let opts = RequestOptions {
        retries: 1,
        timeout: 1,
        min_backoff: 200_000,
        ..RequestOptions::default()
    };
    c.get(DEFAULT_POLICY_ID, &m.url("/timeout/"), Some(opts))
        .unwrap();
    let r = c.wait_for(1, WAIT).pop().unwrap();
    assert_eq!(r.status, Status::easy(sys::CURLE_OPERATION_TIMEDOUT));
    assert_eq!(r.retries, 1);
    assert_eq!(m.records().len(), 2);
    let opts = RequestOptions {
        retries: 0,
        timeout: 10,
        transfer_timeout: 1,
        ..RequestOptions::default()
    };
    c.get(DEFAULT_POLICY_ID, &m.url("/timeout/"), Some(opts))
        .unwrap();
    let r = c.wait_for(1, WAIT).pop().unwrap();
    assert_eq!(r.status, Status::easy(sys::CURLE_OPERATION_TIMEDOUT));
    assert_eq!(r.retries, 0);
    stop(c, &s);
}

#[test]
fn partial_body() {
    let (s, m) = started();
    let mut c = Client::new(&s);
    let opts = RequestOptions {
        retries: 1,
        min_backoff: 200_000,
        ..RequestOptions::default()
    };
    c.get(DEFAULT_POLICY_ID, &m.url("/partial/1000"), Some(opts))
        .unwrap();
    let r = c.wait_for(1, WAIT).pop().unwrap();
    assert_eq!(r.status, Status::easy(sys::CURLE_PARTIAL_FILE));
    assert_eq!(r.retries, 1);
    assert_eq!(r.body.len(), 500);
    c.get(DEFAULT_POLICY_ID, &m.url("/partial/1000"), Some(options(0)))
        .unwrap();
    let r = c.wait_for(1, WAIT).pop().unwrap();
    assert_eq!(r.status, Status::easy(sys::CURLE_PARTIAL_FILE));
    assert_eq!(r.retries, 0);
    stop(c, &s);
}

#[test]
fn redirects() {
    let (s, m) = started();
    let mut c = Client::new(&s);
    c.get(DEFAULT_POLICY_ID, &m.url("/redirect/2"), None)
        .unwrap();
    let r = c.wait_for(1, WAIT).pop().unwrap();
    assert_eq!(r.status, Status::http(200));
    assert_eq!(r.body, LLSD_OK);
    let rec = m.records();
    assert_eq!(
        rec.iter().map(|r| r.target.as_str()).collect::<Vec<_>>(),
        ["/redirect/2", "/redirect/1", "/ok/"]
    );
    assert!(rec[1].header("Referer").unwrap().ends_with("/redirect/2"));
    let opts = RequestOptions {
        follow_redirects: false,
        ..RequestOptions::default()
    };
    c.get(DEFAULT_POLICY_ID, &m.url("/redirect/1"), Some(opts))
        .unwrap();
    let r = c.wait_for(1, WAIT).pop().unwrap();
    assert_eq!(r.status, Status::http(302));
    assert_eq!(m.records().len(), 4);
    stop(c, &s);
}

#[test]
fn concurrency_limits_and_throttle() {
    let mut s = service();
    let a = s.create_policy_class();
    let b = s.create_policy_class();
    let t = s.create_policy_class();
    assert_eq!((a, b, t), (1, 2, 3));
    assert_eq!(
        s.set_option_long(PolicyOption::ConnectionLimit as i32, a, 2),
        Ok(2)
    );
    assert_eq!(
        s.set_option_long(PolicyOption::ConnectionLimit as i32, b, 4),
        Ok(4)
    );
    assert_eq!(
        s.set_option_long(PolicyOption::ThrottleRate as i32, t, 5),
        Ok(5)
    );
    s.start_thread();
    let m = MockServer::start();
    let mut c = Client::new(&s);
    for i in 0..8 {
        c.get(a, &m.url(&format!("/slow/300/A/{i}")), None).unwrap();
        c.get(b, &m.url(&format!("/slow/300/B/{i}")), None).unwrap();
        c.get(t, &m.url(&format!("/slow/300/C/{i}")), None).unwrap();
    }
    let replies = c.wait_for(24, WAIT);
    assert_eq!(replies.len(), 24);
    assert!(replies.iter().all(|r| r.status == Status::http(200)));
    let rec = m.records();
    let overlap = |group: &str| {
        let g: Vec<_> = rec
            .iter()
            .filter(|r| r.target.contains(&format!("/{group}/")))
            .collect();
        g.iter()
            .map(|x| {
                g.iter()
                    .filter(|y| y.t_ms <= x.t_ms && y.answered_ms.unwrap() > x.t_ms)
                    .count()
            })
            .max()
            .unwrap_or(0)
    };
    assert_eq!(overlap("A"), 2);
    assert_eq!(overlap("B"), 4);
    assert_eq!(overlap("C"), 5);
    let mut ct: Vec<f64> = rec
        .iter()
        .filter(|r| r.target.contains("/C/"))
        .map(|r| r.t_ms)
        .collect();
    ct.sort_by(|x, y| x.partial_cmp(y).unwrap());
    let window = ct[5] - ct[0];
    assert!(
        (900.0..1400.0).contains(&window),
        "sixth throttled request after {window} ms"
    );
    stop(c, &s);
}

#[test]
fn cancellation() {
    let (s, m) = started();
    let mut c = Client::new(&s);
    let queued = c
        .get(DEFAULT_POLICY_ID, &m.url("/slow/1500/Q/0"), None)
        .unwrap();
    let cancel = c.cancel(queued).unwrap();
    let replies = c.wait_for(2, WAIT);
    let req = replies.iter().find(|r| r.handle == queued).unwrap();
    let can = replies.iter().find(|r| r.handle == cancel).unwrap();
    assert_eq!(req.status, Status::llcore(he::OP_CANCELED));
    assert_eq!((can.kind, can.status), (ReplyKind::Cancel, Status::OK));
    assert!(m.records().is_empty(), "cancelled before it went out");
    let active = c
        .get(DEFAULT_POLICY_ID, &m.url("/slow/1500/R/0"), None)
        .unwrap();
    std::thread::sleep(Duration::from_millis(400));
    let cancel = c.cancel(active).unwrap();
    let replies = c.wait_for(2, WAIT);
    assert_eq!(
        replies.iter().find(|r| r.handle == active).unwrap().status,
        Status::llcore(he::OP_CANCELED)
    );
    assert_eq!(
        replies.iter().find(|r| r.handle == cancel).unwrap().status,
        Status::OK
    );
    assert_eq!(m.records().len(), 1);
    c.cancel(0x7777).unwrap();
    let r = c.wait_for(1, WAIT).pop().unwrap();
    assert_eq!(r.status, Status::llcore(he::HANDLE_NOT_FOUND));
    stop(c, &s);
}

#[test]
fn headers_cookies_and_conditional() {
    let (s, m) = started();
    let mut c = Client::new(&s);
    let req = Request::new(DEFAULT_POLICY_ID, Method::Get, m.url("/headers/"))
        .header("Accept", "image/x-j2c")
        .header("X-Custom-Header", "custom value")
        .options(RequestOptions {
            want_headers: true,
            ..RequestOptions::default()
        });
    c.request(req).unwrap();
    let r = c.wait_for(1, WAIT).pop().unwrap();
    let headers = r.headers.unwrap();
    assert!(
        headers
            .iter()
            .any(|(n, v)| n == "x-reflect-accept" && v == "image/x-j2c")
    );
    assert!(
        headers
            .iter()
            .any(|(n, v)| n == "x-reflect-x-custom-header" && v == "custom value")
    );
    let rec = m.records();
    assert_eq!(rec[0].header("Accept"), Some("image/x-j2c"));
    // The cookie engine is per easy handle and handles are recycled, so
    // the cookie set on one request is sent on the next.
    c.get(DEFAULT_POLICY_ID, &m.url("/cookie/set"), None)
        .unwrap();
    c.wait_for(1, WAIT);
    c.get(DEFAULT_POLICY_ID, &m.url("/cookie/get"), None)
        .unwrap();
    let r = c.wait_for(1, WAIT).pop().unwrap();
    assert_eq!(r.body, b"cookie:vks=1");
    let opts = RequestOptions {
        last_modified: 1_700_000_000,
        ..RequestOptions::default()
    };
    c.get(DEFAULT_POLICY_ID, &m.url("/headers/"), Some(opts))
        .unwrap();
    c.wait_for(1, WAIT);
    assert!(
        m.records()
            .last()
            .unwrap()
            .header("If-Modified-Since")
            .is_some()
    );
    stop(c, &s);
}

#[test]
fn odd_bodies() {
    let (s, m) = started();
    let mut c = Client::new(&s);
    c.get(DEFAULT_POLICY_ID, &m.url("/chunked/"), None).unwrap();
    let r = c.wait_for(1, WAIT).pop().unwrap();
    assert_eq!(r.body, pattern(1000));
    c.get(DEFAULT_POLICY_ID, &m.url("/nolength/"), None)
        .unwrap();
    let r = c.wait_for(1, WAIT).pop().unwrap();
    assert_eq!(r.body, b"no length body");
    let opts = RequestOptions {
        retries: 1,
        min_backoff: 200_000,
        ..RequestOptions::default()
    };
    c.get_range(DEFAULT_POLICY_ID, &m.url("/badrange/"), 0, 3, Some(opts))
        .unwrap();
    let r = c.wait_for(1, WAIT).pop().unwrap();
    assert_eq!(r.status, Status::llcore(he::INV_CONTENT_RANGE_HDR));
    assert_eq!(r.retries, 1);
    stop(c, &s);
}

#[test]
fn policy_options_static_and_dynamic() {
    let mut s = service();
    let a = s.create_policy_class();
    assert_eq!(
        s.set_option_long(PolicyOption::ConnectionLimit as i32, a, 3),
        Ok(3)
    );
    assert_eq!(
        s.set_option_long(PolicyOption::ConnectionLimit as i32, a, 1000),
        Ok(256)
    );
    assert_eq!(
        s.set_option_long(PolicyOption::PerHostConnectionLimit as i32, a, 500),
        Ok(256)
    );
    assert_eq!(
        s.set_option_string(PolicyOption::CaFile as i32, a, "x"),
        Err(Status::llcore(he::INVALID_ARG))
    );
    assert_eq!(
        s.set_option_long(PolicyOption::ThrottleRate as i32, GLOBAL_POLICY_ID, 5),
        Err(Status::llcore(he::INVALID_ARG))
    );
    assert_eq!(
        s.set_option_long(PolicyOption::ConnectionLimit as i32, 17, 5),
        Err(Status::llcore(he::INVALID_ARG))
    );
    assert_eq!(
        s.set_option_long(PolicyOption::Trace as i32, GLOBAL_POLICY_ID, 99),
        Ok(3)
    );
    assert_eq!(
        s.set_option_long(PolicyOption::Trace as i32, GLOBAL_POLICY_ID, 0),
        Ok(0)
    );
    assert_eq!(
        s.set_option_long(PolicyOption::CaFile as i32, GLOBAL_POLICY_ID, 5),
        Err(Status::llcore(he::INVALID_ARG))
    );
    assert_eq!(
        s.set_option_string(
            PolicyOption::CaFile as i32,
            GLOBAL_POLICY_ID,
            "/nonexistent/ca.crt"
        ),
        Ok("/nonexistent/ca.crt".into())
    );
    assert_eq!(
        s.set_option_callback(
            PolicyOption::SslVerifyCallback as i32,
            GLOBAL_POLICY_ID,
            true
        ),
        Ok(true)
    );
    s.start_thread();
    assert_eq!(
        s.set_option_long(PolicyOption::ConnectionLimit as i32, a, 3),
        Err(Status::llcore(he::OPT_NOT_DYNAMIC))
    );
    assert_eq!(s.create_policy_class(), 0);
    let m = MockServer::start();
    let mut c = Client::new(&s);
    assert!(
        c.set_option_long(PolicyOption::CaFile, GLOBAL_POLICY_ID, 1)
            .is_none()
    );
    assert_eq!(c.status(), Status::llcore(he::INVALID_ARG));
    let h = c
        .set_option_long(PolicyOption::ConnectionLimit, a, 6)
        .unwrap();
    let r = c.wait_for(1, WAIT).pop().unwrap();
    assert_eq!(
        (r.handle, r.kind, r.status, r.long_value),
        (h, ReplyKind::SetGet, Status::OK, 6)
    );
    c.set_option_long(PolicyOption::PipeliningDepth, a, 3)
        .unwrap();
    assert_eq!(c.wait_for(1, WAIT).pop().unwrap().long_value, 3);
    c.set_option_long(PolicyOption::ConnectionLimit, 17, 6)
        .unwrap();
    assert_eq!(
        c.wait_for(1, WAIT).pop().unwrap().status,
        Status::llcore(he::INVALID_ARG)
    );
    c.get(a, &m.url("/ok/"), None).unwrap();
    assert_eq!(c.wait_for(1, WAIT).pop().unwrap().status, Status::http(200));
    stop(c, &s);
}

#[test]
fn shutdown_cancels_pending_requests() {
    let (s, m) = started();
    let mut c = Client::new(&s);
    let h = c
        .get(DEFAULT_POLICY_ID, &m.url("/slow/2000/S/0"), None)
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    c.stop_thread().unwrap();
    let replies = c.wait_for(2, WAIT);
    assert_eq!(
        replies.iter().find(|r| r.handle == h).unwrap().status,
        Status::llcore(he::OP_CANCELED)
    );
    assert!(s.is_stopped());
    let r = c.get(DEFAULT_POLICY_ID, &m.url("/ok/"), None);
    assert!(r.is_none());
    assert_eq!(c.status(), Status::llcore(he::SHUTTING_DOWN));
}

#[test]
fn status_strings() {
    sys::global_init();
    assert_eq!(
        Status::easy(sys::CURLE_COULDNT_CONNECT).message(),
        "Couldn't connect to server"
    );
    assert_eq!(Status::http(404).message(), "Not Found");
    assert_eq!(
        Status::llcore(he::OP_CANCELED).message(),
        "Operation canceled"
    );
    assert!(!sys::version().is_empty());
}
