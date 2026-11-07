//! The service against the loopback mock server: the scenarios of the
//! viewer's `INTEGRATION_TEST_llcorehttp_parity`, at the crate level.

mod mock;

use std::sync::Arc;
use std::time::Duration;

use mock::{LLSD_OK, MockServer, encoding_text, pattern};
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
    // Accept-Encoding as libcurl 7.54.1 sent it, the transport decoding the
    // reply itself (crate::decode), as the C++.
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

// A deep-queue class (pipelining depth above 1) keeps the connections it
// had with HTTP/1.1 pipelining and runs one request on each: libcurl 8 has
// no pipelining and the transport speaks HTTP/1.1, so the rest wait in the
// policy's queue, never in libcurl's, where the wait for a connection would
// count against the connect timeout. As the C++ with curl 8.
#[test]
fn deep_queue_runs_one_request_per_connection() {
    let mut s = service();
    let d = s.create_policy_class();
    assert_eq!(
        s.set_option_long(PolicyOption::ConnectionLimit as i32, d, 4),
        Ok(4)
    );
    assert_eq!(
        s.set_option_long(PolicyOption::PerHostConnectionLimit as i32, d, 2),
        Ok(2)
    );
    assert_eq!(
        s.set_option_long(PolicyOption::PipeliningDepth as i32, d, 3),
        Ok(3)
    );
    s.start_thread();
    let m = MockServer::start();
    let mut c = Client::new(&s);
    for i in 0..12 {
        c.get(d, &m.url(&format!("/slow/300/D/{i}")), None).unwrap();
    }
    let replies = c.wait_for(12, WAIT);
    assert_eq!(replies.len(), 12);
    assert!(replies.iter().all(|r| r.status == Status::http(200)));
    let rec = m.records();
    let overlapping = |x: &mock::Recorded, y: &mock::Recorded| {
        y.t_ms <= x.t_ms && y.answered_ms.unwrap() > x.t_ms
    };
    let overlap = rec
        .iter()
        .map(|x| rec.iter().filter(|y| overlapping(x, y)).count())
        .max()
        .unwrap_or(0);
    assert_eq!(overlap, 2, "one request per connection, per-host limit 2");
    for x in &rec {
        for y in &rec {
            if x.seq != y.seq && x.conn == y.conn {
                assert!(
                    !overlapping(x, y),
                    "two requests at once on connection {}",
                    x.conn
                );
            }
        }
    }
    let connections: std::collections::HashSet<u32> = rec.iter().map(|r| r.conn).collect();
    assert!(
        connections.len() <= 2,
        "{} connections, per-host limit 2",
        connections.len()
    );
    stop(c, &s);
}

// Per-host admission: a deep-queue class runs up to its per-host limit to
// any one host and its connection limit in all, so a busy host does not
// hold back another one (the texture and mesh2 classes keep 16 in all, 8
// per host, for region crossings). 127.0.0.1 and localhost are two hosts
// to the class. As the C++ HttpPolicy::processReadyQueue with curl 8.
#[test]
fn deep_queue_admits_per_host() {
    let mut s = service();
    let d = s.create_policy_class();
    assert_eq!(
        s.set_option_long(PolicyOption::ConnectionLimit as i32, d, 4),
        Ok(4)
    );
    assert_eq!(
        s.set_option_long(PolicyOption::PerHostConnectionLimit as i32, d, 2),
        Ok(2)
    );
    assert_eq!(
        s.set_option_long(PolicyOption::PipeliningDepth as i32, d, 3),
        Ok(3)
    );
    s.start_thread();
    let m = MockServer::start();
    let mut c = Client::new(&s);
    let other = m.url("").replace("127.0.0.1", "localhost");
    for i in 0..6 {
        c.get(d, &m.url(&format!("/slow/300/H/{i}")), None).unwrap();
    }
    for i in 0..6 {
        c.get(d, &format!("{other}/slow/300/L/{i}"), None).unwrap();
    }
    let replies = c.wait_for(12, WAIT);
    assert_eq!(replies.len(), 12);
    assert!(replies.iter().all(|r| r.status == Status::http(200)));
    let rec = m.records();
    let overlapping = |x: &mock::Recorded, y: &mock::Recorded| {
        y.t_ms <= x.t_ms && y.answered_ms.unwrap() > x.t_ms
    };
    let in_flight = |pick: &dyn Fn(&mock::Recorded) -> bool| {
        rec.iter()
            .filter(|x| pick(x))
            .map(|x| rec.iter().filter(|y| pick(y) && overlapping(x, y)).count())
            .max()
            .unwrap_or(0)
    };
    let to_localhost = |r: &mock::Recorded| r.target.contains("/L/");
    let to_ip = |r: &mock::Recorded| r.target.contains("/H/");
    assert_eq!(in_flight(&to_ip), 2, "per-host limit 2 to 127.0.0.1");
    assert_eq!(in_flight(&to_localhost), 2, "per-host limit 2 to localhost");
    assert_eq!(
        in_flight(&|_| true),
        4,
        "both hosts at once, connection limit 4"
    );
    stop(c, &s);
}

// Options apply at once, also under active requests (the stall that let
// a class go idle first protected libcurl 7.37's pipelining switch): turning
// a deep-queue class's depth to 0 while its requests run loses or fails
// none of them, and later requests run under the new limit (the connection
// limit, 4).
#[test]
fn options_change_under_active_requests() {
    let mut s = service();
    let d = s.create_policy_class();
    assert_eq!(
        s.set_option_long(PolicyOption::ConnectionLimit as i32, d, 4),
        Ok(4)
    );
    assert_eq!(
        s.set_option_long(PolicyOption::PerHostConnectionLimit as i32, d, 2),
        Ok(2)
    );
    assert_eq!(
        s.set_option_long(PolicyOption::PipeliningDepth as i32, d, 3),
        Ok(3)
    );
    s.start_thread();
    let m = MockServer::start();
    let mut c = Client::new(&s);
    for i in 0..6 {
        c.get(d, &m.url(&format!("/slow/600/E/{i}")), None).unwrap();
    }
    // Let the first two (one per connection, per-host limit 2) get under
    // way, then switch while they run.
    let start = std::time::Instant::now();
    while m.records().len() < 2 && start.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        m.records().len(),
        2,
        "two requests in flight before the switch"
    );
    let h = c
        .set_option_long(PolicyOption::PipeliningDepth, d, 0)
        .unwrap();
    for i in 0..8 {
        c.get(d, &m.url(&format!("/slow/200/F/{i}")), None).unwrap();
    }
    let replies = c.wait_for(15, WAIT);
    assert_eq!(replies.len(), 15);
    let set = replies.iter().find(|r| r.handle == h).unwrap();
    assert_eq!(
        (set.kind, set.status, set.long_value),
        (ReplyKind::SetGet, Status::OK, 0)
    );
    assert!(
        replies
            .iter()
            .filter(|r| r.handle != h)
            .all(|r| r.status == Status::http(200)),
        "every request completes"
    );
    let rec = m.records();
    let f: Vec<_> = rec.iter().filter(|r| r.target.contains("/F/")).collect();
    assert_eq!(f.len(), 8);
    let overlap = f
        .iter()
        .map(|x| {
            f.iter()
                .filter(|y| y.t_ms <= x.t_ms && y.answered_ms.unwrap() > x.t_ms)
                .count()
        })
        .max()
        .unwrap_or(0);
    assert!(
        overlap <= 4,
        "{overlap} in flight after the switch, limit 4"
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
    c.get_range(
        DEFAULT_POLICY_ID,
        &m.url("/badrange/"),
        0,
        3,
        Some(opts.clone()),
    )
    .unwrap();
    let r = c.wait_for(1, WAIT).pop().unwrap();
    assert_eq!(r.status, Status::llcore(he::INV_CONTENT_RANGE_HDR));
    assert_eq!(r.retries, 1);
    // A 206 that starts elsewhere than asked is refused (retryable); one
    // that starts where asked is delivered, even if longer (BUG-2295).
    c.get_range(
        DEFAULT_POLICY_ID,
        &m.url("/fixedrange/"),
        20,
        10,
        Some(opts.clone()),
    )
    .unwrap();
    let r = c.wait_for(1, WAIT).pop().unwrap();
    assert_eq!(r.status, Status::llcore(he::INV_CONTENT_RANGE_HDR));
    assert_eq!(r.retries, 1);
    c.get_range(
        DEFAULT_POLICY_ID,
        &m.url("/fixedrange/"),
        0,
        5,
        Some(opts.clone()),
    )
    .unwrap();
    let r = c.wait_for(1, WAIT).pop().unwrap();
    assert_eq!(r.status, Status::http(206));
    assert_eq!(r.body, pattern(10));
    // A 206 without a usable Content-Range reads as offset 0: refused for a
    // request at a later offset, delivered for one at offset 0.
    for path in ["/norange/", "/oddrange/"] {
        c.get_range(DEFAULT_POLICY_ID, &m.url(path), 20, 10, Some(opts.clone()))
            .unwrap();
        let r = c.wait_for(1, WAIT).pop().unwrap();
        assert_eq!(
            r.status,
            Status::llcore(he::INV_CONTENT_RANGE_HDR),
            "{path}"
        );
        assert_eq!(r.retries, 1, "{path}");
        c.get_range(DEFAULT_POLICY_ID, &m.url(path), 0, 10, Some(opts.clone()))
            .unwrap();
        let r = c.wait_for(1, WAIT).pop().unwrap();
        assert_eq!(r.status, Status::http(206), "{path}");
        assert_eq!(r.body, pattern(10), "{path}");
    }
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
    // libcurl's own text: curl 8 says "Could not", 7.54.1 said "Couldn't".
    let couldnt = Status::easy(sys::CURLE_COULDNT_CONNECT).message();
    assert!(
        couldnt == "Could not connect to server" || couldnt == "Couldn't connect to server",
        "{couldnt}"
    );
    assert_eq!(Status::http(404).message(), "Not Found");
    assert_eq!(
        Status::llcore(he::OP_CANCELED).message(),
        "Operation canceled"
    );
    assert!(!sys::version().is_empty());
}

// Content-Encoding as libcurl 7.54.1 handled it and the C++ llcorehttp does
// (its test 24): gzip (gzip or zlib wrapper, several members, trailing
// bytes, raw deflate, the first of a coding list) and deflate (zlib or raw)
// are undone, any other value passes through, and gzip data that does not decode fails with
// CURLE_BAD_CONTENT_ENCODING (61).
#[test]
fn content_encoding() {
    let (s, m) = started();
    let mut c = Client::new(&s);
    let text = encoding_text();
    for (kind, ok) in [
        ("gzip", true),
        ("gzipzlib", true),
        ("gzipmulti", true),
        ("gziptrail", true),
        ("deflate", true),
        ("rawdeflate", true),
        ("gzipraw", true),
        ("gziplist", true),
        ("binary", true),
        ("none", true),
        ("badgzip", false),
    ] {
        c.get(
            DEFAULT_POLICY_ID,
            &m.url(&format!("/encoding/{kind}/")),
            None,
        )
        .unwrap();
        let r = c.wait_for(1, WAIT).pop().unwrap();
        if ok {
            assert_eq!(r.status, Status::http(200), "{kind}");
            assert_eq!(r.body, text, "{kind}: decoded body");
        } else {
            assert_eq!(r.status, Status::easy(61), "{kind}: refused");
        }
    }
    stop(c, &s);
}
