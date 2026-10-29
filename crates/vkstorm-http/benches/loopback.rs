//! Requests per second and latency per request against the loopback mock
//! server, single client, one request in flight at a time: a small LLSD
//! GET, a small LLSD POST (echoed) and a 1 MiB range download. The
//! viewer-level counterpart (`INTEGRATION_TEST_llcorehttp_parity` with
//! `VKS_HTTP_BENCH`) measures the same through the C++ classes and the C
//! ABI; this harness measures the crate alone.
//!
//! `VKS_HTTP_BENCH_ITERATIONS` sets the iteration count (default 2000).

#[path = "../tests/mock/mod.rs"]
mod mock;

use std::sync::Arc;
use std::time::{Duration, Instant};

use mock::MockServer;
use vkstorm_http::{
    Client, DEFAULT_POLICY_ID, Method, NoHooks, Request, Service, TEST_USER_AGENT, sys,
};

const LLSD_BODY: &[u8] = b"<?xml version=\"1.0\" ?><llsd><map><key>agent_id</key><uuid>0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0</uuid><key>message</key><string>parity</string><key>count</key><integer>42</integer></map></llsd>";

fn run(
    name: &str,
    client: &mut Client<'_>,
    count: usize,
    mut issue: impl FnMut(&mut Client<'_>) -> u64,
) {
    let mut lat: Vec<f64> = Vec::with_capacity(count);
    let start = Instant::now();
    for _ in 0..count {
        let t = Instant::now();
        let h = issue(client);
        loop {
            if let Some(r) = client.fetch() {
                assert_eq!(r.handle, h);
                assert!(r.status.is_ok(), "{:?}", r.status);
                break;
            }
            std::thread::yield_now();
        }
        lat.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    let total = start.elapsed().as_secs_f64();
    lat.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mean = lat.iter().sum::<f64>() / lat.len() as f64;
    let median = lat[lat.len() / 2];
    let p95 = lat[(lat.len() as f64 * 0.95) as usize];
    println!(
        "| {name} | {:.0} | {mean:.3} | {median:.3} | {p95:.3} |",
        count as f64 / total
    );
}

fn main() {
    sys::global_init();
    let iterations: usize = std::env::var("VKS_HTTP_BENCH_ITERATIONS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2000);
    let mut service = Service::new(Arc::new(NoHooks), TEST_USER_AGENT);
    service.start_thread();
    let server = MockServer::start();
    let mut client = Client::new(&service);
    // Warm-up: a connection and a cached handle.
    for _ in 0..3 {
        client
            .get(DEFAULT_POLICY_ID, &server.url("/ok/"), None)
            .unwrap();
        client.wait_for(1, Duration::from_secs(10));
    }
    println!("libcurl: {}", sys::version());
    println!("| operation | requests/s | mean ms | median ms | p95 ms |");
    println!("|---|---:|---:|---:|---:|");
    let ok = server.url("/ok/");
    run("GET small LLSD (125 B)", &mut client, iterations, |c| {
        c.request(
            Request::new(DEFAULT_POLICY_ID, Method::Get, ok.clone())
                .header("Accept", "application/llsd+xml"),
        )
        .unwrap()
    });
    let echo = server.url("/echo/");
    run(
        "POST small LLSD (191 B, echoed)",
        &mut client,
        iterations,
        |c| {
            c.request(
                Request::new(DEFAULT_POLICY_ID, Method::Post, echo.clone())
                    .body(LLSD_BODY.to_vec())
                    .header("Content-Type", "application/llsd+xml")
                    .header("Accept", "application/llsd+xml"),
            )
            .unwrap()
        },
    );
    let large = server.url("/large/1048576");
    run(
        "GET 1 MiB range",
        &mut client,
        (iterations / 10).max(1),
        |c| {
            c.get_range(DEFAULT_POLICY_ID, &large, 0, 1_048_576, None)
                .unwrap()
        },
    );
    client.stop_thread().unwrap();
    let start = Instant::now();
    while !service.is_stopped() && start.elapsed() < Duration::from_secs(10) {
        client.update();
        std::thread::sleep(Duration::from_millis(1));
    }
}
