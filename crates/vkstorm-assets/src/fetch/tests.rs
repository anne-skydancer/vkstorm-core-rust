//! The state machine against a scripted host: the decision sequences the
//! viewer's integration test compares against its recording, here for the
//! crate's own regression coverage.

use super::*;
use super::budget::MAX_IMAGE_DATA_SIZE;

#[derive(Debug, Clone, PartialEq)]
enum Call {
    ReadCache(Option<String>, i32, i32),
    SendHttp(HttpGet),
    Decode(i32, bool),
    WriteCache(i32, i32),
    RemoveFromCache,
    DropFormatted,
    ResetFormatted,
}

struct MockHost {
    env: Env,
    region: Option<Region>,
    calls: Vec<Call>,
    http_ok: bool,
    decode_ok: bool,
    next_decode_handle: u32,
    formatted: Vec<u8>,
    formatted_discard: i32,
    http_body: Vec<u8>,
    logs: Vec<String>,
}

impl MockHost {
    fn new() -> MockHost {
        MockHost {
            env: Env { can_load_from_cache: true, use_http_textures: true, ..Default::default() },
            region: Some(Region { id: [1; 16], viewer_asset_url: "http://asset.example".into(), http_url: String::new() }),
            calls: Vec::new(),
            http_ok: true,
            decode_ok: true,
            next_decode_handle: 10,
            formatted: Vec::new(),
            formatted_discard: -1,
            http_body: Vec::new(),
            logs: Vec::new(),
        }
    }
}

impl Host for MockHost {
    fn env(&mut self) -> Env {
        let mut e = self.env.clone();
        e.formatted_present = !self.formatted.is_empty() || self.formatted_discard >= 0;
        e.formatted_size = self.formatted.len() as i32;
        e.formatted_discard = self.formatted_discard;
        e.formatted_is_j2c = true;
        e
    }
    fn region(&mut self) -> Option<Region> {
        self.region.clone()
    }
    fn read_cache(&mut self, local_file: Option<&str>, offset: i32, size: i32) {
        self.calls.push(Call::ReadCache(local_file.map(|s| s.to_string()), offset, size));
    }
    fn cache_read_complete(&mut self) -> bool {
        true
    }
    fn send_http(&mut self, get: &HttpGet) -> bool {
        self.calls.push(Call::SendHttp(get.clone()));
        self.http_ok
    }
    fn drop_raw_images(&mut self) {}
    fn decode(&mut self, discard: i32, needs_aux: bool) -> u32 {
        self.calls.push(Call::Decode(discard, needs_aux));
        if self.decode_ok {
            self.next_decode_handle += 1;
            self.next_decode_handle
        } else {
            0
        }
    }
    fn write_cache(&mut self, file_size: i32, decoded_discard: i32) {
        self.calls.push(Call::WriteCache(file_size, decoded_discard));
    }
    fn cache_write_complete(&mut self) -> bool {
        true
    }
    fn prioritize_write(&mut self) {}
    fn remove_from_cache(&mut self) {
        self.calls.push(Call::RemoveFromCache);
    }
    fn reset_formatted_data(&mut self) {
        self.calls.push(Call::ResetFormatted);
        self.formatted.clear();
        self.http_body.clear();
    }
    fn drop_formatted_image(&mut self) {
        self.calls.push(Call::DropFormatted);
        self.formatted.clear();
        self.formatted_discard = -1;
    }
    fn release_http_buffer(&mut self) {
        self.http_body.clear();
    }
    fn assemble_http_reply(&mut self, _url: &str, src_offset: i32, cur_size: i32, append_size: i32, total_size: i32) -> bool {
        let mut out = Vec::with_capacity(total_size as usize);
        out.extend_from_slice(&self.formatted[..cur_size as usize]);
        out.extend_from_slice(&self.http_body[src_offset as usize..(src_offset + append_size) as usize]);
        assert_eq!(out.len(), total_size as usize);
        self.formatted = out;
        self.formatted_discard = 0;
        self.http_body.clear();
        true
    }
    fn add_to_http_queue(&mut self) {}
    fn record_texture_start(&mut self, _is_http: bool) {}
    fn record_texture_done(&mut self, _is_http: bool, _byte_count: f64) {}
    fn stat_cache_attempt(&mut self) {}
    fn stat_cache_hit(&mut self) {}
    fn stat_cache_hit_rate(&mut self, _hit: bool) {}
    fn log(&mut self, _level: i32, _once: bool, msg: &str) {
        self.logs.push(msg.to_string());
    }
    #[cfg(feature = "opensim")]
    fn add_to_network_queue(&mut self) {}
    #[cfg(feature = "opensim")]
    fn remove_from_network_queue(&mut self, _cancel: bool) {}
    #[cfg(feature = "opensim")]
    fn assemble_packets(&mut self, _first_packet: i32, _last_packet: i32) -> i32 {
        0
    }
}

fn new_worker(discard: i32, size: i32) -> Worker {
    let mut w = Worker::new(FetchType::Default, "", [9; 16], false, 1000.0, true);
    w.set_desired_discard(discard, size, false);
    w
}

#[test]
fn cache_miss_then_http_then_decode_then_write() {
    let mut f = Fetcher::new();
    let mut h = MockHost::new();
    let mut w = new_worker(2, 5000);
    // INIT -> cache read issued, wait.
    assert!(!w.do_work(&mut f, &mut h));
    assert_eq!(h.calls, vec![Call::ReadCache(None, 0, 5000)]);
    assert_eq!(w.state, State::LoadFromTextureCache);
    // Cache miss: the responder reports failure.
    w.cache_read_done(false, 0, 0, -1, false);
    // -> CACHE_POST -> LOAD_FROM_NETWORK -> WAIT_HTTP_RESOURCE -> SEND -> WAIT_HTTP_REQ
    assert!(!w.do_work(&mut f, &mut h));
    assert_eq!(w.state, State::WaitHttpReq);
    assert_eq!(f.http_semaphore, 1);
    assert_eq!(w.url, "http://asset.example/?texture_id=09090909-0909-0909-0909-090909090909");
    assert_eq!(
        h.calls[1],
        Call::SendHttp(HttpGet { url: w.url.clone(), offset: 0, size: 5000, no_range: false, with_headers: false })
    );
    // A 206 with 5000 bytes.
    h.http_body = vec![7; 5000];
    let n = w.http_completed(&mut f, &mut h, true, 206, "Partial Content", true, 5000, Some((0, 5000, 100000)), None);
    assert_eq!(n, Some(5000));
    assert!(!w.have_all_data);
    assert!(!w.do_work(&mut f, &mut h));
    assert_eq!(w.state, State::DecodeImageUpdate);
    assert_eq!(f.http_semaphore, 0);
    assert_eq!(w.file_size, 5001);
    assert_eq!(h.calls.last(), Some(&Call::Decode(2, false)));
    let handle = w.decode_handle;
    h.formatted_discard = 2;
    assert!(w.decode_done(true, 2, handle, &mut h));
    assert!(!w.do_work(&mut f, &mut h));
    assert_eq!(w.state, State::WaitOnWrite);
    assert_eq!(h.calls.last(), Some(&Call::WriteCache(5001, 2)));
    w.cache_write_done(true);
    assert!(w.do_work(&mut f, &mut h));
    assert_eq!(w.state, State::Done);
    assert_eq!(w.decoded_discard, 2);
}

#[test]
fn cache_hit_decodes_without_http() {
    let mut f = Fetcher::new();
    let mut h = MockHost::new();
    let mut w = new_worker(3, 2000);
    assert!(!w.do_work(&mut f, &mut h));
    h.formatted = vec![1; 2600];
    h.formatted_discard = 0;
    w.cache_read_done(true, 2600, 2600, 0, false);
    assert!(!w.do_work(&mut f, &mut h));
    assert_eq!(w.state, State::DecodeImageUpdate);
    assert!(w.in_cache);
    assert!(w.have_all_data);
    assert_eq!(w.write_to_cache_state, WriteToCacheState::NotWrite);
    assert_eq!(h.calls, vec![Call::ReadCache(None, 0, 2000), Call::Decode(3, false)]);
    let handle = w.decode_handle;
    assert!(w.decode_done(true, 3, handle, &mut h));
    assert!(w.do_work(&mut f, &mut h));
    assert_eq!(w.state, State::Done);
    assert!(!h.calls.iter().any(|c| matches!(c, Call::WriteCache(..))));
}

#[test]
fn partial_cache_then_range_from_cached_bytes() {
    let mut f = Fetcher::new();
    let mut h = MockHost::new();
    let mut w = new_worker(1, 20000);
    assert!(!w.do_work(&mut f, &mut h));
    // 3000 bytes cached of a larger image.
    h.formatted = vec![1; 3000];
    h.formatted_discard = 3;
    w.cache_read_done(true, 3001, 3000, 0, false);
    assert!(!w.do_work(&mut f, &mut h));
    assert_eq!(w.state, State::WaitHttpReq);
    // The range starts one byte before the data held (offset - 1, size + 1).
    assert_eq!(
        h.calls.last(),
        Some(&Call::SendHttp(HttpGet { url: w.url.clone(), offset: 2999, size: 17001, no_range: false, with_headers: false }))
    );
    h.http_body = vec![2; 17001];
    w.http_completed(&mut f, &mut h, true, 206, "", true, 17001, Some((2999, 17001, 100000)), None);
    assert!(!w.do_work(&mut f, &mut h));
    assert_eq!(h.formatted.len(), 20000);
    assert_eq!(w.requested_offset, 3000);
    assert_eq!(w.requested_size, 17000);
    assert_eq!(w.state, State::DecodeImageUpdate);
}

#[test]
fn short_reply_means_all_data() {
    let mut f = Fetcher::new();
    let mut h = MockHost::new();
    let mut w = new_worker(0, MAX_IMAGE_DATA_SIZE);
    assert!(!w.do_work(&mut f, &mut h));
    w.cache_read_done(false, 0, 0, -1, false);
    assert!(!w.do_work(&mut f, &mut h));
    // The range would end past the 20 MB cap: ask for the rest.
    assert_eq!(
        h.calls.last(),
        Some(&Call::SendHttp(HttpGet { url: w.url.clone(), offset: 0, size: 0, no_range: false, with_headers: false }))
    );
    h.http_body = vec![3; 4321];
    w.http_completed(&mut f, &mut h, true, 206, "", true, 4321, Some((0, 4321, 4321)), None);
    assert!(w.have_all_data);
    assert!(!w.do_work(&mut f, &mut h));
    assert_eq!(w.file_size, 4321);
    assert_eq!(h.calls.last(), Some(&Call::Decode(0, false)));
}

#[test]
fn not_found_retries_once_on_region_change_then_fails() {
    let mut f = Fetcher::new();
    let mut h = MockHost::new();
    let mut w = new_worker(2, 5000);
    assert!(!w.do_work(&mut f, &mut h));
    w.cache_read_done(false, 0, 0, -1, false);
    assert!(!w.do_work(&mut f, &mut h));
    let url1 = w.url.clone();
    w.http_completed(&mut f, &mut h, false, 404, "Not Found", false, 0, None, None);
    // Same region: no retry; the URL is cleared, the fetch fails harder.
    assert!(w.do_work(&mut f, &mut h));
    assert_eq!(w.state, State::Done);
    assert!(w.url.is_empty());
    assert_eq!(f.http_semaphore, 0);
    assert_eq!(h.calls.last(), Some(&Call::ResetFormatted));
    assert!(h.logs.iter().any(|l| l.contains("fail harder")));

    // A new worker whose region changed between request and reply retries.
    let mut h = MockHost::new();
    let mut w = new_worker(2, 5000);
    assert!(!w.do_work(&mut f, &mut h));
    w.cache_read_done(false, 0, 0, -1, false);
    assert!(!w.do_work(&mut f, &mut h));
    assert_eq!(w.url, url1);
    h.region = Some(Region { id: [2; 16], viewer_asset_url: "http://asset2.example".into(), http_url: String::new() });
    w.http_completed(&mut f, &mut h, false, 404, "Not Found", false, 0, None, None);
    assert!(!w.do_work(&mut f, &mut h));
    assert_eq!(w.state, State::Init);
    assert_eq!(w.retry_attempt, 1);
    assert!(w.url.is_empty());
    // The semaphore is still held across the retry, as in the C++.
    assert_eq!(f.http_semaphore, 1);
    assert!(w.http_has_resource);
    assert!(!w.do_work(&mut f, &mut h));
    assert_eq!(h.calls.last(), Some(&Call::ReadCache(None, 0, 5000)));
    w.cache_read_done(false, 0, 0, -1, false);
    assert!(!w.do_work(&mut f, &mut h));
    assert_eq!(w.state, State::WaitHttpReq);
    assert!(w.url.starts_with("http://asset2.example/?texture_id="));
    assert_eq!(f.http_semaphore, 1);
}

#[test]
fn range_not_satisfiable_with_data_decodes() {
    let mut f = Fetcher::new();
    let mut h = MockHost::new();
    let mut w = new_worker(0, 50000);
    assert!(!w.do_work(&mut f, &mut h));
    h.formatted = vec![1; 4000];
    h.formatted_discard = 2;
    w.cache_read_done(true, 4001, 4000, 0, false);
    assert!(!w.do_work(&mut f, &mut h));
    assert_eq!(w.state, State::WaitHttpReq);
    w.http_completed(&mut f, &mut h, false, 416, "Requested Range Not Satisfiable", false, 0, None, None);
    assert!(!w.do_work(&mut f, &mut h));
    assert!(w.have_all_data);
    assert_eq!(w.loaded_discard, 2);
    assert_eq!(w.state, State::DecodeImageUpdate);
    assert_eq!(h.calls.last(), Some(&Call::Decode(2, false)));
}

#[test]
fn server_busy_without_region_change_fails_harder() {
    let mut f = Fetcher::new();
    let mut h = MockHost::new();
    let mut w = new_worker(2, 5000);
    assert!(!w.do_work(&mut f, &mut h));
    w.cache_read_done(false, 0, 0, -1, false);
    assert!(!w.do_work(&mut f, &mut h));
    w.http_completed(&mut f, &mut h, false, 503, "Service Unavailable", false, 0, None, None);
    assert!(w.do_work(&mut f, &mut h));
    assert_eq!(w.state, State::Done);
}

#[test]
fn decode_failure_of_cached_data_retries_from_network() {
    let mut f = Fetcher::new();
    let mut h = MockHost::new();
    let mut w = new_worker(2, 2000);
    assert!(!w.do_work(&mut f, &mut h));
    h.formatted = vec![1; 2600];
    h.formatted_discard = 0;
    w.cache_read_done(true, 2600, 2600, 0, false);
    assert!(!w.do_work(&mut f, &mut h));
    let handle = w.decode_handle;
    assert!(w.decode_done(false, -1, handle, &mut h));
    assert_eq!(h.calls.last(), Some(&Call::RemoveFromCache));
    // Retry: back to INIT, the formatted image dropped, a new cache read.
    assert!(!w.do_work(&mut f, &mut h));
    assert_eq!(w.retry_attempt, 1);
    assert_eq!(w.state, State::LoadFromTextureCache);
    assert_eq!(h.calls.last(), Some(&Call::ReadCache(None, 0, 2000)));
    w.cache_read_done(false, 0, 0, -1, false);
    assert!(!w.do_work(&mut f, &mut h));
    assert_eq!(w.state, State::WaitHttpReq);
}

#[test]
fn obsolete_decode_callbacks_are_ignored() {
    let mut f = Fetcher::new();
    let mut h = MockHost::new();
    let mut w = new_worker(2, 2000);
    assert!(!w.do_work(&mut f, &mut h));
    h.formatted = vec![1; 2600];
    h.formatted_discard = 0;
    w.cache_read_done(true, 2600, 2600, 0, false);
    assert!(!w.do_work(&mut f, &mut h));
    let handle = w.decode_handle;
    assert!(!w.decode_done(true, 2, handle + 5, &mut h));
    assert!(!w.decoded);
    assert!(w.decode_done(true, 2, handle, &mut h));
    assert!(w.decoded);
}

#[test]
fn resource_waiters_release_by_priority() {
    let mut f = Fetcher::new();
    f.http_high_water = 1;
    f.http_low_water = 1;
    let mut hosts: Vec<MockHost> = (0..3).map(|_| MockHost::new()).collect();
    let mut workers: Vec<Worker> = (0..3u8)
        .map(|i| {
            let mut w = Worker::new(FetchType::Default, "", [i + 1; 16], false, (i as f32 + 1.0) * 100.0, true);
            w.set_desired_discard(2, 5000, false);
            w
        })
        .collect();
    for (w, h) in workers.iter_mut().zip(hosts.iter_mut()) {
        assert!(!w.do_work(&mut f, h));
        w.cache_read_done(false, 0, 0, -1, false);
        assert!(!w.do_work(&mut f, h));
    }
    // One got the slot, two wait.
    assert_eq!(f.http_semaphore, 1);
    assert_eq!(workers[0].state, State::WaitHttpReq);
    assert_eq!(workers[1].state, State::WaitHttpResource2);
    assert_eq!(workers[2].state, State::WaitHttpResource2);
    assert_eq!(f.http_waiters_count(), 2);
    // Nothing to release while the semaphore is at the low water.
    assert_eq!(f.waiters_to_release().0, 0);
    hosts[0].http_body = vec![1; 5000];
    workers[0].http_completed(&mut f, &mut hosts[0], true, 206, "", true, 5000, Some((0, 5000, 9000)), None);
    assert!(!workers[0].do_work(&mut f, &mut hosts[0]));
    assert_eq!(f.http_semaphore, 0);
    let (needed, ids) = f.waiters_to_release();
    assert_eq!(needed, 1);
    assert_eq!(ids.len(), 2);
    // The highest priority waiter is released first; the next finds no slot.
    let prios: Vec<f32> = ids.iter().map(|id| workers.iter().find(|w| &w.id == id).unwrap().image_priority).collect();
    let order = Fetcher::release_order(&prios, needed);
    assert_eq!(prios[order[0]], 300.0);
    let mut released = Vec::new();
    for &k in &order {
        let id = ids[k];
        let w = workers.iter_mut().find(|w| w.id == id).unwrap();
        match w.try_release(&mut f) {
            Release::Released => {
                f.remove_http_waiter(&id);
                released.push(id);
            }
            Release::NotWaiting => f.remove_http_waiter(&id),
            Release::NoSlot => break,
        }
    }
    assert_eq!(released, vec![[3; 16]]);
    assert_eq!(f.http_semaphore, 1);
    assert_eq!(f.http_waiters_count(), 1);
    assert_eq!(workers[2].state, State::SendHttpReq);
    assert_eq!(workers[1].state, State::WaitHttpResource2);
}

#[test]
fn desired_size_planning() {
    assert_eq!(plan::desired_size(FetchType::Default, false, false, 0, 0, 0, 5), (FIRST_PACKET_SIZE, MAX_DISCARD_LEVEL));
    assert_eq!(plan::desired_size(FetchType::Default, false, false, 512, 512, 3, 0), (MAX_IMAGE_DATA_SIZE, 0));
    assert_eq!(plan::desired_size(FetchType::Default, true, true, 512, 512, 3, 3), (MAX_IMAGE_DATA_SIZE, 0));
    let (size, d) = plan::desired_size(FetchType::Default, false, false, 512, 512, 3, 3);
    assert_eq!(d, 3);
    assert_eq!(size, budget::calc_data_size_j2c(512, 512, 3, 3, budget::DEFAULT_RATE));
}

#[test]
fn zero_priority_aborts_before_network() {
    let mut f = Fetcher::new();
    let mut h = MockHost::new();
    let mut w = new_worker(2, 5000);
    w.set_image_priority(0.0);
    assert!(w.do_work(&mut f, &mut h));
    assert!(h.calls.is_empty());
}

#[test]
fn state_names() {
    assert_eq!(State::string_for(9), "WAIT_HTTP_REQ");
    assert_eq!(State::string_for(-3), "-3");
    assert_eq!(State::from_i32(14), State::Done);
}
