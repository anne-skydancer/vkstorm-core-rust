//! A scripted loopback HTTP/1.1 server for the crate's tests and benches:
//! the same paths as the viewer's `vkshttp_mock_server.py` (less gzip),
//! recording every request it receives. Raw sockets, one thread per
//! connection, keep-alive, no external network.

#![allow(dead_code)]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub const LLSD_OK: &[u8] = b"<?xml version=\"1.0\" ?><llsd><map><key>reply</key><string>success</string><key>status</key><integer>200</integer></map></llsd>";

/// Deterministic body of n bytes (the Python server's `pattern`).
pub fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| (((i * 7 + 13) ^ (i >> 8)) & 0xFF) as u8).collect()
}

#[derive(Clone, Debug)]
pub struct Recorded {
    pub t_ms: f64,
    pub conn: u32,
    pub seq: u32,
    pub method: String,
    pub target: String,
    pub version: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub raw_head: Vec<u8>,
    pub answered_ms: Option<f64>,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

struct Shared {
    start: Instant,
    records: Mutex<Vec<Recorded>>,
    flaky: Mutex<HashMap<String, u32>>,
    conn_counter: AtomicU32,
    stop: AtomicBool,
}

pub struct MockServer {
    shared: Arc<Shared>,
    pub port: u16,
    thread: Option<thread::JoinHandle<()>>,
}

impl MockServer {
    pub fn start() -> MockServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        listener.set_nonblocking(true).expect("nonblocking");
        let port = listener.local_addr().unwrap().port();
        let shared = Arc::new(Shared {
            start: Instant::now(),
            records: Mutex::new(Vec::new()),
            flaky: Mutex::new(HashMap::new()),
            conn_counter: AtomicU32::new(0),
            stop: AtomicBool::new(false),
        });
        let s2 = shared.clone();
        let thread = thread::spawn(move || {
            while !s2.stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((sock, _)) => {
                        let _ = sock.set_nonblocking(false);
                        let _ = sock.set_nodelay(true);
                        let s3 = s2.clone();
                        thread::spawn(move || Connection::new(sock, s3).run());
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(5)),
                    Err(_) => break,
                }
            }
        });
        MockServer { shared, port, thread: Some(thread) }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
    }

    pub fn records(&self) -> Vec<Recorded> {
        self.shared.records.lock().unwrap().clone()
    }

    pub fn clear(&self) {
        self.shared.records.lock().unwrap().clear();
    }

    pub fn elapsed_ms(&self) -> f64 {
        self.shared.start.elapsed().as_secs_f64() * 1000.0
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Connection {
    sock: TcpStream,
    shared: Arc<Shared>,
    conn: u32,
    seq: u32,
    buf: Vec<u8>,
}

struct Req {
    method: String,
    target: String,
    version: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    raw_head: Vec<u8>,
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

impl Connection {
    fn new(sock: TcpStream, shared: Arc<Shared>) -> Connection {
        let conn = shared.conn_counter.fetch_add(1, Ordering::Relaxed) + 1;
        Connection { sock, shared, conn, seq: 0, buf: Vec::new() }
    }

    fn fill(&mut self) -> bool {
        let mut tmp = [0u8; 65536];
        match self.sock.read(&mut tmp) {
            Ok(0) | Err(_) => false,
            Ok(n) => {
                self.buf.extend_from_slice(&tmp[..n]);
                true
            }
        }
    }

    fn recv_until(&mut self, marker: &[u8]) -> bool {
        while find(&self.buf, marker).is_none() {
            if !self.fill() {
                return false;
            }
        }
        true
    }

    fn recv_exact(&mut self, n: usize) -> bool {
        while self.buf.len() < n {
            if !self.fill() {
                return false;
            }
        }
        true
    }

    fn read_request(&mut self) -> Option<Req> {
        if !self.recv_until(b"\r\n\r\n") {
            return None;
        }
        let end = find(&self.buf, b"\r\n\r\n").unwrap();
        let raw_head: Vec<u8> = self.buf[..end + 4].to_vec();
        self.buf.drain(..end + 4);
        let head = String::from_utf8_lossy(&raw_head[..end]).into_owned();
        let mut lines = head.split("\r\n");
        let request_line = lines.next().unwrap_or("");
        let mut parts = request_line.split(' ');
        let method = parts.next().unwrap_or("").to_owned();
        let target = parts.next().unwrap_or("").to_owned();
        let version = parts.next().unwrap_or("").to_owned();
        let mut headers = Vec::new();
        for line in lines {
            if line.is_empty() {
                continue;
            }
            let (n, v) = line.split_once(':').unwrap_or((line, ""));
            headers.push((n.to_owned(), v.trim().to_owned()));
        }
        let mut body = Vec::new();
        let te = headers.iter().find(|(n, _)| n.eq_ignore_ascii_case("transfer-encoding")).map(|(_, v)| v.to_ascii_lowercase());
        let cl = headers.iter().find(|(n, _)| n.eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.parse::<usize>().ok());
        if te.as_deref().is_some_and(|t| t.contains("chunked")) {
            loop {
                if !self.recv_until(b"\r\n") {
                    return None;
                }
                let p = find(&self.buf, b"\r\n").unwrap();
                let line = String::from_utf8_lossy(&self.buf[..p]).into_owned();
                self.buf.drain(..p + 2);
                let size = usize::from_str_radix(line.split(';').next().unwrap_or("0").trim(), 16).unwrap_or(0);
                if size == 0 {
                    if !self.recv_until(b"\r\n") {
                        return None;
                    }
                    let p = find(&self.buf, b"\r\n").unwrap();
                    self.buf.drain(..p + 2);
                    break;
                }
                if !self.recv_exact(size + 2) {
                    return None;
                }
                body.extend_from_slice(&self.buf[..size]);
                self.buf.drain(..size + 2);
            }
        } else if let Some(n) = cl {
            if !self.recv_exact(n) {
                return None;
            }
            body.extend_from_slice(&self.buf[..n]);
            self.buf.drain(..n);
        }
        Some(Req { method, target, version, headers, body, raw_head })
    }

    fn run(mut self) {
        while let Some(req) = self.read_request() {
            self.seq += 1;
            let t_ms = self.shared.start.elapsed().as_secs_f64() * 1000.0;
            let index;
            {
                let mut recs = self.shared.records.lock().unwrap();
                index = recs.len();
                recs.push(Recorded {
                    t_ms,
                    conn: self.conn,
                    seq: self.seq,
                    method: req.method.clone(),
                    target: req.target.clone(),
                    version: req.version.clone(),
                    headers: req.headers.clone(),
                    body: req.body.clone(),
                    raw_head: req.raw_head.clone(),
                    answered_ms: None,
                });
            }
            let keep = self.respond(&req);
            {
                let mut recs = self.shared.records.lock().unwrap();
                if let Some(r) = recs.get_mut(index) {
                    r.answered_ms = Some(self.shared.start.elapsed().as_secs_f64() * 1000.0);
                }
            }
            if !keep {
                break;
            }
        }
        let _ = self.sock.shutdown(Shutdown::Both);
    }

    fn send(&mut self, status: u32, reason: &str, headers: &[(&str, String)], body: &[u8], content_length: bool, close: bool) -> bool {
        let mut head = format!("HTTP/1.1 {status} {reason}\r\n");
        if content_length && status != 204 && status != 304 {
            head.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        if close {
            head.push_str("Connection: close\r\n");
        }
        for (n, v) in headers {
            head.push_str(&format!("{n}: {v}\r\n"));
        }
        head.push_str("\r\n");
        let mut out = head.into_bytes();
        if status != 204 && status != 304 {
            out.extend_from_slice(body);
        }
        self.sock.write_all(&out).is_ok()
    }

    fn respond(&mut self, req: &Req) -> bool {
        let path = req.target.split('?').next().unwrap_or("").to_owned();
        let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        let kind = parts.first().copied().unwrap_or("");
        let ct = |v: &str| ("Content-Type", v.to_owned());
        match kind {
            "mark" => self.send(200, "OK", &[ct("text/plain")], b"", true, false),
            "ok" => self.send(200, "OK", &[ct("application/llsd+xml")], LLSD_OK, true, false),
            "large" => {
                let n: usize = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
                let full = pattern(n);
                let range = req.headers.iter().find(|(h, _)| h.eq_ignore_ascii_case("range")).map(|(_, v)| v.clone());
                match range.as_deref().and_then(|r| r.strip_prefix("bytes=")) {
                    Some(spec) => {
                        let (a, b) = spec.split_once('-').unwrap_or((spec, ""));
                        let first: usize = a.parse().unwrap_or(0);
                        let last: usize = if b.is_empty() { n.saturating_sub(1) } else { b.parse().unwrap_or(0) };
                        if first >= n {
                            self.send(416, "Range Not Satisfiable", &[("Content-Range", format!("bytes */{n}")), ct("text/plain")], b"out of range", true, false)
                        } else {
                            let last = last.min(n - 1);
                            self.send(
                                206,
                                "Partial Content",
                                &[("Content-Range", format!("bytes {first}-{last}/{n}")), ct("application/octet-stream")],
                                &full[first..=last],
                                true,
                                false,
                            )
                        }
                    }
                    None => self.send(200, "OK", &[ct("application/octet-stream")], &full, true, false),
                }
            }
            "echo" => {
                let ctype = req.headers.iter().find(|(h, _)| h.eq_ignore_ascii_case("content-type")).map(|(_, v)| v.clone()).unwrap_or_else(|| "text/plain".into());
                let body = req.body.clone();
                self.send(200, "OK", &[("Content-Type", ctype), ("X-Echo-Method", req.method.clone())], &body, true, false)
            }
            "status" => {
                let code: u32 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(500);
                self.send(code, "Status", &[ct("text/plain")], format!("status {code}").as_bytes(), true, false)
            }
            "flaky" => {
                let key = parts.get(1).copied().unwrap_or("").to_owned();
                let limit: u32 = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
                let retry = if parts.get(3) == Some(&"retry") { parts.get(4).map(|s| s.to_string()) } else { None };
                let count = {
                    let mut f = self.shared.flaky.lock().unwrap();
                    let c = f.entry(key).or_insert(0);
                    let v = *c;
                    *c += 1;
                    v
                };
                if count < limit {
                    let mut hdrs = vec![ct("text/plain")];
                    if let Some(r) = retry {
                        hdrs.push(("Retry-After", r));
                    }
                    self.send(503, "Service Unavailable", &hdrs, b"flaky", true, false)
                } else {
                    self.send(200, "OK", &[ct("application/llsd+xml")], LLSD_OK, true, false)
                }
            }
            "timeout" => {
                let mut tmp = [0u8; 4096];
                while matches!(self.sock.read(&mut tmp), Ok(n) if n > 0) {}
                false
            }
            "partial" | "reset" => {
                let n: usize = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
                let full = pattern(n);
                let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {n}\r\nContent-Type: application/octet-stream\r\n\r\n");
                let mut out = head.into_bytes();
                out.extend_from_slice(&full[..n / 2]);
                let _ = self.sock.write_all(&out);
                // No RST here: std has no stable SO_LINGER. The viewer's
                // Python mock covers the reset case; "reset" closes cleanly.
                thread::sleep(Duration::from_millis(50));
                false
            }
            "redirect" => {
                let n: u32 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(1);
                let location = if n <= 1 { "/ok/".to_owned() } else { format!("/redirect/{}", n - 1) };
                self.send(302, "Found", &[("Location", location), ct("text/plain")], b"moved", true, false)
            }
            "slow" => {
                let ms: u64 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
                thread::sleep(Duration::from_millis(ms));
                self.send(200, "OK", &[ct("application/llsd+xml")], LLSD_OK, true, false)
            }
            "headers" => {
                let hdrs: Vec<(String, String)> = req.headers.iter().map(|(n, v)| (format!("X-Reflect-{n}"), v.clone())).collect();
                let mut refs: Vec<(&str, String)> = hdrs.iter().map(|(n, v)| (n.as_str(), v.clone())).collect();
                refs.push(ct("text/plain"));
                self.send(200, "OK", &refs, b"reflected", true, false)
            }
            "cookie" => {
                if parts.get(1) == Some(&"set") {
                    self.send(200, "OK", &[("Set-Cookie", "vks=1; Path=/".into()), ct("text/plain")], b"set", true, false)
                } else {
                    let c = req.headers.iter().find(|(h, _)| h.eq_ignore_ascii_case("cookie")).map(|(_, v)| v.clone()).unwrap_or_default();
                    self.send(200, "OK", &[ct("text/plain")], format!("cookie:{c}").as_bytes(), true, false)
                }
            }
            "nolength" => {
                self.send(200, "OK", &[ct("text/plain")], b"no length body", false, true);
                false
            }
            "badrange" => self.send(206, "Partial Content", &[("Content-Range", "bytes 10-5/100".into()), ct("text/plain")], b"bad", true, false),
            "chunked" => {
                let body = pattern(1000);
                let mut out = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Type: application/octet-stream\r\n\r\n".to_vec();
                for piece in body.chunks(300) {
                    out.extend_from_slice(format!("{:x}\r\n", piece.len()).as_bytes());
                    out.extend_from_slice(piece);
                    out.extend_from_slice(b"\r\n");
                }
                out.extend_from_slice(b"0\r\n\r\n");
                self.sock.write_all(&out).is_ok()
            }
            _ => self.send(404, "Not Found", &[ct("text/plain")], b"not found", true, false),
        }
    }
}
