//! The texture cache script of the viewer's parity test
//! (`indra/newview/tests/lltexturecache_parity_test.cpp`) replayed through
//! the crate alone, against the files that test recorded from the C++
//! cache before the port (`tests/fixtures/lltexturecache_parity.bin`, a
//! copy of the viewer's fixture).
//!
//! The script is the same sequence of writes, reads, fast-cache reads,
//! re-writes, removals, an over-limit purge, a reopen with body validation
//! and a version-mismatch purge, with the same generator and the same
//! fixed clock. Every file is compared byte for byte, except the pixel
//! bytes of the fast cache records (the viewer scales the raw image with
//! `LLImageRaw::scale`, which stays in C++; their 16-byte heads are
//! compared) and the entries header's engine string (the codec's, not the
//! cache's).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use vkstorm_assets::Uuid;
use vkstorm_assets::texcache::{
    Config, FastEntry, LAZY_PURGE_TIME_LIMIT, Paths, TextureCache, fast_cache_level,
};

const FIXTURE_MAGIC: u32 = 0x5643_5450;
const FIXTURE_VERSION: u32 = 1;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u32) -> u32 {
        (self.next() % n as u64) as u32
    }
    fn uuid(&mut self) -> Uuid {
        let mut id = [0u8; 16];
        for b in id.iter_mut() {
            *b = self.next() as u8;
        }
        id
    }
}

static CLOCK: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1_700_000_000);

fn fixed_clock() -> u32 {
    CLOCK.load(std::sync::atomic::Ordering::Relaxed)
}

fn clock_add(d: u32) {
    CLOCK.fetch_add(d, std::sync::atomic::Ordering::Relaxed);
}

#[derive(Clone)]
struct Texture {
    id: Uuid,
    data: Vec<u8>,
    image_size: i32,
    discard: i32,
    raw_w: i32,
    raw_h: i32,
    raw_c: i32,
}

fn make_data(rng: &mut Rng, size: i32) -> Vec<u8> {
    (0..size).map(|_| (rng.next() >> 8) as u8).collect()
}

/// `make_raw` consumes one generator value per pixel byte; the pixels
/// themselves are not compared.
fn consume_raw(rng: &mut Rng, w: i32, h: i32, c: i32) {
    for _ in 0..(w * h * c) {
        rng.next();
    }
}

fn write_texture(cache: &TextureCache, rng: &mut Rng, t: &Texture) -> bool {
    consume_raw(rng, t.raw_w, t.raw_h, t.raw_c);
    // The viewer's wrapper: the lazy purge slice, then the fast-cache
    // record at the level that fits.
    cache.lazy_purge_tick(LAZY_PURGE_TIME_LIMIT);
    let level = fast_cache_level(t.raw_w, t.raw_h, t.raw_c);
    let (mut w, mut h, mut discard) = (t.raw_w, t.raw_h, t.discard);
    if level > 0 {
        w >>= level;
        h >>= level;
        if w * h * t.raw_c > 0 {
            discard += level;
        }
    }
    let fast = FastEntry {
        width: w,
        height: h,
        components: t.raw_c,
        discard_level: discard,
        data: vec![0; (w * h * t.raw_c).max(0) as usize],
    };
    cache.write(&t.id, &t.data, t.image_size, t.discard, &fast)
}

type Snapshot = BTreeMap<String, Vec<u8>>;

fn snapshot(root: &Path) -> Snapshot {
    let mut out = BTreeMap::new();
    let tex = root.join("texturecache");
    if let Ok(rd) = fs::read_dir(&tex) {
        for e in rd.flatten() {
            if e.file_type().map(|t| t.is_file()).unwrap_or(false) {
                out.insert(
                    format!("texturecache/{}", e.file_name().to_string_lossy()),
                    fs::read(e.path()).unwrap(),
                );
            }
        }
    }
    for c in "0123456789abcdef".chars() {
        let sub = tex.join(c.to_string());
        if let Ok(rd) = fs::read_dir(&sub) {
            for e in rd.flatten() {
                if e.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    out.insert(
                        format!("texturecache/{}/{}", c, e.file_name().to_string_lossy()),
                        fs::read(e.path()).unwrap(),
                    );
                }
            }
        }
    }
    out
}

fn run_script(root: &Path) -> Vec<(String, Snapshot)> {
    let delim = std::path::MAIN_SEPARATOR.to_string();
    let paths = Paths::for_location(&root.to_string_lossy(), &delim);
    let mut rng = Rng(0x9E3779B97F4A7C15);
    CLOCK.store(1_700_000_000, std::sync::atomic::Ordering::Relaxed);
    let mut cfg = Config::new("replay-engine", 64);
    cfg.clock = fixed_clock;

    let mut textures: Vec<Texture> = Vec::new();
    for i in 0..40 {
        let mut id = rng.uuid();
        if i < 3 {
            id[0] = 0;
        }
        let size = if i % 2 == 0 {
            100 + rng.below(499) as i32
        } else {
            601 + rng.below(19000) as i32
        };
        let data = make_data(&mut rng, size);
        let image_size = size
            + if rng.below(3) == 0 {
                1 + rng.below(5000) as i32
            } else {
                0
            };
        let discard = rng.below(6) as i32;
        let dims = [16, 32, 64, 128];
        let raw_w = dims[rng.below(4) as usize];
        let raw_h = dims[rng.below(4) as usize];
        let raw_c = if rng.below(2) == 0 { 3 } else { 4 };
        textures.push(Texture {
            id,
            data,
            image_size,
            discard,
            raw_w,
            raw_h,
            raw_c,
        });
    }

    let mut snapshots = Vec::new();
    {
        let cache = TextureCache::new(cfg.clone());
        cache.set_read_only(false);
        cache.init_cache(&paths, 500_000, false, Some(0));
        for t in textures.iter() {
            clock_add(1 + rng.below(3));
            write_texture(&cache, &mut rng, t);
        }
        for i in (0..textures.len()).step_by(3) {
            let _ = cache.read(&textures[i].id, 0, textures[i].data.len() as i32 + 4096);
            let _ = cache.read(&textures[i].id, 0, 300);
        }
        let _ = cache.read(&rng.uuid(), 0, 4096);
        for i in (0..textures.len()).step_by(5) {
            let _ = cache.read_fast(&textures[i].id);
        }
        let mut i = 1;
        while i < textures.len() {
            clock_add(2);
            let mut t = textures[i].clone();
            let more = t.data.len() as i32 + 700 + rng.below(3000) as i32;
            t.data = make_data(&mut rng, more);
            t.image_size = more + 1;
            textures[i] = t.clone();
            write_texture(&cache, &mut rng, &t);
            write_texture(&cache, &mut rng, &t);
            i += 4;
        }
        let mut i = 2;
        while i < textures.len() {
            cache.remove(&textures[i].id);
            i += 7;
        }
        cache.remove(&rng.uuid());
        for _ in 0..24 {
            clock_add(1);
            let id = rng.uuid();
            let size = 15000 + rng.below(6000) as i32;
            let data = make_data(&mut rng, size);
            let t = Texture {
                id,
                data,
                image_size: size,
                discard: 1,
                raw_w: 64,
                raw_h: 64,
                raw_c: 3,
            };
            write_texture(&cache, &mut rng, &t);
        }
        for _ in 0..12 {
            let t = textures[0].clone();
            write_texture(&cache, &mut rng, &t);
        }
        cache.shutdown();
    }
    snapshots.push(("after_script".to_string(), snapshot(root)));

    {
        let tex = root.join("texturecache").join("0");
        let _ = fs::remove_file(tex.join(format!(
            "{}.texture",
            vkstorm_assets::uuid_to_string(&textures[1].id)
        )));
        fs::write(
            tex.join(format!(
                "{}.texture",
                vkstorm_assets::uuid_to_string(&textures[2].id)
            )),
            b"short",
        )
        .unwrap();
        let cache = TextureCache::new(cfg.clone());
        cache.set_read_only(false);
        cache.init_cache(&paths, 500_000, false, Some(0));
        for t in textures.iter().take(3) {
            let _ = cache.is_in_cache(&t.id);
        }
        let _ = cache.read(&textures[2].id, 0, 4096);
        let _ = cache.read(&textures[0].id, 0, 4096);
        cache.shutdown();
    }
    snapshots.push(("after_reopen".to_string(), snapshot(root)));

    {
        let cache = TextureCache::new(cfg);
        cache.set_read_only(false);
        cache.init_cache(&paths, 500_000, true, Some(1));
        cache.shutdown();
    }
    snapshots.push(("after_mismatch".to_string(), snapshot(root)));
    snapshots
}

fn read_fixture(bytes: &[u8]) -> Vec<(String, Snapshot)> {
    let mut pos = 0usize;
    let u32 = |pos: &mut usize| {
        let v = u32::from_le_bytes(bytes[*pos..*pos + 4].try_into().unwrap());
        *pos += 4;
        v
    };
    assert_eq!(u32(&mut pos), FIXTURE_MAGIC);
    assert_eq!(u32(&mut pos), FIXTURE_VERSION);
    let snaps = u32(&mut pos);
    let mut out = Vec::new();
    for _ in 0..snaps {
        let n = u32(&mut pos) as usize;
        let name = String::from_utf8_lossy(&bytes[pos..pos + n]).into_owned();
        pos += n;
        let files = u32(&mut pos);
        let mut snap = BTreeMap::new();
        for _ in 0..files {
            let n = u32(&mut pos) as usize;
            let rel = String::from_utf8_lossy(&bytes[pos..pos + n]).into_owned();
            pos += n;
            let size = u32(&mut pos) as usize;
            snap.insert(rel, bytes[pos..pos + size].to_vec());
            pos += size;
        }
        out.push((name, snap));
    }
    out
}

/// Masks what the crate cannot reproduce byte for byte: the fast cache
/// pixels (scaled by the viewer) and the entries header's engine string.
fn mask(rel: &str, bytes: &[u8]) -> Vec<u8> {
    let mut b = bytes.to_vec();
    if rel == "texturecache/FastCache.cache" {
        let mut off = 0;
        while off + 1040 <= b.len() {
            for v in b[off + 16..off + 1040].iter_mut() {
                *v = 0;
            }
            off += 1040;
        }
    } else if rel == "texturecache/texture.entries" && b.len() >= 44 {
        for v in b[8..40].iter_mut() {
            *v = 0;
        }
    }
    b
}

#[test]
fn replay_matches_recorded_files() {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lltexturecache_parity.bin");
    let want = read_fixture(&fs::read(fixture).expect("fixture present"));
    let root = std::env::temp_dir().join(format!("vks_texcache_replay_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let got = run_script(&root);
    assert_eq!(got.len(), want.len());
    let mut problems = Vec::new();
    let mut files = 0usize;
    for ((gname, gsnap), (wname, wsnap)) in got.iter().zip(want.iter()) {
        assert_eq!(gname, wname);
        for (rel, wbytes) in wsnap {
            files += 1;
            match gsnap.get(rel) {
                None => problems.push(format!("{wname}: missing {rel}")),
                Some(gbytes) => {
                    if mask(rel, gbytes) != mask(rel, wbytes) {
                        let n = gbytes.len().min(wbytes.len());
                        let k = (0..n).find(|&i| gbytes[i] != wbytes[i]).unwrap_or(n);
                        problems.push(format!(
                            "{wname}: {rel} differs at byte {k} (sizes {} vs {})",
                            gbytes.len(),
                            wbytes.len()
                        ));
                    }
                }
            }
        }
        for rel in gsnap.keys() {
            if !wsnap.contains_key(rel) {
                problems.push(format!("{wname}: unexpected {rel}"));
            }
        }
    }
    for p in &problems {
        eprintln!("PARITY: {p}");
    }
    eprintln!(
        "compared {files} files over {} snapshots, {} differences",
        want.len(),
        problems.len()
    );
    let _ = fs::remove_dir_all(&root);
    assert!(problems.is_empty());
}
