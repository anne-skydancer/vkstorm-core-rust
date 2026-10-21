//! The crate-side timings of the hot paths the viewer's integration tests
//! also time (`VKS_MESH_BENCH`, `VKS_TEXCACHE_BENCH`, `VKS_TEXFETCH_BENCH`):
//! unpacking the largest mesh LOD of the corpus, a texture cache index
//! lookup, write and read, and assembling a 1 MiB texture from range
//! replies through the fetch state machine with the host's work mocked.
//! Run with `cargo bench --bench hot_paths -- [iterations]`; the C++ tests
//! print the same quantities for the comparison table in doc/rust_port.md.

#![allow(dead_code)]

#[path = "../tests/mesh_corpus.rs"]
mod corpus;

use std::time::Instant;

use vkstorm_assets::fetch::{budget, Env, FetchType, Fetcher, Host, HttpGet, Region, State, Worker};
use vkstorm_assets::mesh::skin::{Decomposition, SkinInfo};
use vkstorm_assets::mesh::unpack::unpack_volume_faces;
use vkstorm_assets::texcache::{fast_cache_level, Config, FastEntry, Paths, TextureCache};
use vkstorm_assets::Uuid;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn uuid(&mut self) -> Uuid {
        let mut u = [0u8; 16];
        for b in u.iter_mut() {
            *b = self.next() as u8;
        }
        u
    }
    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
}

fn min_ms<F: FnMut()>(iterations: usize, mut f: F) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..iterations {
        let t = Instant::now();
        f();
        best = best.min(t.elapsed().as_secs_f64() * 1000.0);
    }
    best
}

// ---- the mesh corpus: the largest high LOD, all parts ----------------------

fn bench_mesh(iterations: usize) {
    let bytes = std::fs::read(corpus::fixture_path()).expect("corpus fixture present");
    let cases = corpus::read_corpus(&bytes);
    let mut best: Option<(&corpus::Case, usize)> = None;
    for c in &cases {
        let tris: usize = c.raw_faces[3].iter().map(|f| f.num_indices as usize / 3).sum();
        if best.map(|(_, t)| tris > t).unwrap_or(true) {
            best = Some((c, tris));
        }
    }
    let (c, tris) = best.expect("a case");
    let (header_size, parts, skin, convex) = corpus::parse_asset_header(&c.asset);
    let ms = min_ms(iterations, || {
        for p in 0..corpus::NUM_PARTS {
            if let Some((offset, size)) = parts[p] {
                if offset >= 0 && size > 0 {
                    let start = header_size + offset as usize;
                    let block = &c.asset[start..start + size as usize];
                    let sculpt = if p == 4 { 5 } else { c.sculpt_type };
                    let faces = unpack_volume_faces(block, sculpt).expect("unpacks");
                    std::hint::black_box(faces);
                }
            }
        }
        if let Some((offset, size)) = skin {
            if offset >= 0 && size > 0 {
                let start = header_size + offset as usize;
                std::hint::black_box(SkinInfo::from_zipped(&c.asset[start..start + size as usize]).ok());
            }
        }
        if let Some((offset, size)) = convex {
            if offset >= 0 && size > 0 {
                let start = header_size + offset as usize;
                std::hint::black_box(Decomposition::from_zipped(&c.asset[start..start + size as usize]).ok());
            }
        }
    });
    println!("mesh,unpack_all_parts,{},{},{:.4}", c.name, tris, ms);
}

// ---- the texture cache: lookup, write, read ----------------------------------

fn bench_texcache(iterations: usize) {
    let root = std::env::temp_dir().join("vks_assets_bench_texcache");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let delim = std::path::MAIN_SEPARATOR.to_string();
    let paths = Paths::for_location(&root.to_string_lossy(), &delim);
    let cache = TextureCache::new(Config::new("bench", 64));
    cache.set_read_only(false);
    cache.init_cache(&paths, 200 * 1024 * 1024, false, None);
    let mut rng = Rng(42);
    let level = fast_cache_level(64, 64, 3);
    let fast_dim = 64 >> level;
    let fast = |rng: &mut Rng| FastEntry { width: fast_dim, height: fast_dim, components: 3, discard_level: 2 + level, data: rng.bytes((fast_dim * fast_dim * 3) as usize) };
    let mut ids = Vec::new();
    let mut datas = Vec::new();
    for _ in 0..200 {
        let id = rng.uuid();
        let data = rng.bytes(4000);
        let f = fast(&mut rng);
        assert!(cache.write(&id, &data, 4000, 2, &f));
        ids.push(id);
        datas.push(data);
    }
    // Lookups: the index lookup under the header lock.
    let t = Instant::now();
    let mut hits = 0;
    for _ in 0..iterations {
        for id in &ids {
            hits += cache.is_in_cache(id) as usize;
        }
    }
    let lookup_us = t.elapsed().as_secs_f64() * 1e6 / (iterations * ids.len()) as f64;
    // Writes of a new 4000-byte texture (index insert, entry write, fast
    // cache record, header record, body file).
    let t = Instant::now();
    let mut writes = 0;
    for it in 0..iterations {
        let id = rng.uuid();
        let f = fast(&mut rng);
        writes += cache.write(&id, &datas[it % datas.len()], 4000, 2, &f) as usize;
    }
    let write_us = t.elapsed().as_secs_f64() * 1e6 / iterations as f64;
    // Reads of a cached 4000-byte texture (index lookup, header record and
    // body file reads).
    let t = Instant::now();
    for it in 0..iterations {
        std::hint::black_box(cache.read(&ids[it % ids.len()], 0, 8192));
    }
    let read_us = t.elapsed().as_secs_f64() * 1e6 / iterations as f64;
    println!("texcache,lookup,{:.3}", lookup_us);
    println!("texcache,write,{:.3}", write_us);
    println!("texcache,read,{:.3}", read_us);
    eprintln!("texcache hits={hits} writes={writes}");
    cache.shutdown();
    let _ = std::fs::remove_dir_all(&root);
}

// ---- the fetch state machine: a 1 MiB texture over six discard levels -----
// (a 2048x2048 RGB as far as the byte budget is concerned)

/// The host with the viewer's work mocked: the "file" is a byte buffer the
/// range replies come from; the cache never hits; decodes and cache writes
/// complete at once. What remains is the state machine and the assembly
/// of the formatted image from the replies.
struct BenchHost {
    file: Vec<u8>,
    formatted: Vec<u8>,
    reply: Option<(usize, usize)>, // offset, size of the pending reply
    pending_get: Option<HttpGet>,
    cache_read: bool,
    decode: bool,
    write: bool,
    decode_handle: u32,
}

impl Host for BenchHost {
    fn env(&mut self) -> Env {
        Env {
            quitting: false,
            delete_requested: false,
            debug_pause: false,
            non_interactive: false,
            blacklisted: false,
            formatted_present: !self.formatted.is_empty(),
            formatted_size: self.formatted.len() as i32,
            // As LLImageJ2C reports it after updateData: the discard level
            // the bytes held are good for.
            formatted_discard: if self.formatted.is_empty() { -1 } else { budget::calc_discard_level_bytes(IMAGE_DIM, IMAGE_DIM, 3, self.formatted.len() as i32, REVERSE_BYTE_RANGE_PERCENT) },
            formatted_is_j2c: true,
            can_load_from_cache: true,
            use_http_textures: true,
            disconnected: false,
            range_requests_disabled: false,
            decode_disabled: false,
            texture_bandwidth: 0.0,
            max_bandwidth: 1500.0,
            raw_width: IMAGE_DIM,
            raw_height: IMAGE_DIM,
            formatted_width: IMAGE_DIM,
            formatted_height: IMAGE_DIM,
        }
    }
    fn region(&mut self) -> Option<Region> {
        Some(Region { id: [1; 16], viewer_asset_url: "http://bench/asset".to_string(), http_url: String::new() })
    }
    fn read_cache(&mut self, _local_file: Option<&str>, _offset: i32, _size: i32) {
        self.cache_read = true;
    }
    fn cache_read_complete(&mut self) -> bool {
        true
    }
    fn send_http(&mut self, get: &HttpGet) -> bool {
        self.pending_get = Some(get.clone());
        true
    }
    fn drop_raw_images(&mut self) {}
    fn decode(&mut self, _discard: i32, _needs_aux: bool) -> u32 {
        self.decode = true;
        self.decode_handle += 1;
        self.decode_handle
    }
    fn write_cache(&mut self, _file_size: i32, _decoded_discard: i32) {
        self.write = true;
    }
    fn cache_write_complete(&mut self) -> bool {
        true
    }
    fn prioritize_write(&mut self) {}
    fn remove_from_cache(&mut self) {}
    fn reset_formatted_data(&mut self) {
        self.formatted.clear();
        self.reply = None;
    }
    fn drop_formatted_image(&mut self) {
        self.formatted.clear();
    }
    fn release_http_buffer(&mut self) {
        self.reply = None;
    }
    fn assemble_http_reply(&mut self, _url: &str, src_offset: i32, cur_size: i32, append_size: i32, total_size: i32) -> bool {
        // As the viewer's host: a new buffer of total_size, the existing
        // bytes copied in, then append_size bytes of the reply from
        // src_offset.
        let (offset, _) = self.reply.expect("a reply");
        let mut buffer = Vec::with_capacity(total_size as usize);
        buffer.extend_from_slice(&self.formatted[..cur_size as usize]);
        let from = offset + src_offset as usize;
        buffer.extend_from_slice(&self.file[from..from + append_size as usize]);
        self.formatted = buffer;
        self.reply = None;
        true
    }
    fn add_to_http_queue(&mut self) {}
    fn record_texture_start(&mut self, _is_http: bool) {}
    fn record_texture_done(&mut self, _is_http: bool, _byte_count: f64) {}
    fn stat_cache_attempt(&mut self) {}
    fn stat_cache_hit(&mut self) {}
    fn stat_cache_hit_rate(&mut self, _hit: bool) {}
    fn log(&mut self, _level: i32, _once: bool, _msg: &str) {}
    #[cfg(feature = "opensim")]
    fn add_to_network_queue(&mut self) {}
    #[cfg(feature = "opensim")]
    fn remove_from_network_queue(&mut self, _cancel: bool) {}
    #[cfg(feature = "opensim")]
    fn assemble_packets(&mut self, _first_packet: i32, _last_packet: i32) -> i32 {
        0
    }
}

/// The viewer's TextureReverseByteRange default.
const REVERSE_BYTE_RANGE_PERCENT: i32 = 50;
/// The image the 1 MiB stands for: large enough that its discard-0 byte
/// budget exceeds the file, so the last level fetches the rest.
const IMAGE_DIM: i32 = 2048;

fn bench_fetch(iterations: usize) {
    const FILE_SIZE: usize = 1024 * 1024;
    let mut rng = Rng(7);
    let file = rng.bytes(FILE_SIZE);
    let id = [0x10u8; 16];
    let mut steps = 0usize;
    let ms = min_ms(iterations, || {
        let mut host = BenchHost { file: file.clone(), formatted: Vec::new(), reply: None, pending_get: None, cache_read: false, decode: false, write: false, decode_handle: 0 };
        let mut fetcher = Fetcher::new();
        let mut worker = Worker::new(FetchType::Default, "", id, false, 1000.0, true);
        steps = 0;
        for discard in (0..=5).rev() {
            let size = if discard == 0 { budget::MAX_IMAGE_DATA_SIZE } else { budget::calc_data_size_j2c(IMAGE_DIM, IMAGE_DIM, 3, discard, budget::DEFAULT_RATE) };
            worker.set_desired_discard(discard, size, true);
            if worker.state == State::Done {
                worker.set_state(State::Init);
            }
            let mut guard = 0;
            loop {
                let done = worker.do_work(&mut fetcher, &mut host);
                steps += 1;
                if host.cache_read {
                    host.cache_read = false;
                    worker.cache_read_done(false, 0, 0, 0, false);
                    continue;
                }
                if let Some(get) = host.pending_get.take() {
                    let offset = get.offset as usize;
                    let want = if get.size == 0 { FILE_SIZE - offset } else { get.size as usize };
                    let len = want.min(FILE_SIZE - offset);
                    host.reply = Some((offset, len));
                    let partial = !(offset == 0 && len == FILE_SIZE);
                    let status = if partial { 206 } else { 200 };
                    worker.http_completed(&mut fetcher, &mut host, true, status, "", partial, len as i32, Some((offset as u32, len as u32, FILE_SIZE as u32)), None);
                    continue;
                }
                if host.decode {
                    host.decode = false;
                    worker.decode_done(true, discard, host.decode_handle, &mut host);
                    continue;
                }
                if host.write {
                    host.write = false;
                    worker.cache_write_done(true);
                    continue;
                }
                if done {
                    break;
                }
                guard += 1;
                assert!(guard < 1000, "the machine is stuck in {:?}", worker.state);
            }
            assert_eq!(worker.state, State::Done);
        }
        assert_eq!(host.formatted.len(), FILE_SIZE, "the whole texture was assembled");
        std::hint::black_box(&host.formatted);
    });
    println!("fetch,assemble_1mib_six_levels,{},{:.4}", steps, ms);
}

fn main() {
    let iterations: usize = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(20);
    bench_mesh(iterations);
    bench_texcache(iterations.max(100));
    bench_fetch(iterations);
}
