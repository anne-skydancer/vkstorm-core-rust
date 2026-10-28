//! R2 C1 cache files, the realistic set and the Library cache: regenerates
//! from the crate's cache codec ([`vkstorm_inventory::cache`]) every line of
//! `fixtures/cache/c1_cache_files.txt`, `c1_realistic.txt` and
//! `library_real.txt` that is a function of `loadFromFile`/`saveToFile`,
//! with the committed cache bytes as input:
//!
//! - per committed file: its size and SHA-256, the version header and last
//!   byte, the decode (`rv`, `obsolete`, counts, `cats_to_update`, every
//!   record as `dump_category`/`dump_item`), the re-encode (identity, first
//!   difference, size), the notation rendering beside it (`.notation.txt`),
//!   and the library digest the other states repeat;
//! - the realistic 100k set from its committed `.gz` (decompressed SHA-256,
//!   decode digests per 10,000 records, re-encode identity), `validate()`
//!   of the populated model rebuilt from that cache, and the reload: the
//!   model summary and `loadSkeleton` return values of a login from the
//!   cache (the crate's login load), and `validate()` after
//!   `buildParentChildMap` (rebuilt: nothing is lost in this world);
//! - the Library cache (decode digests per 1,000 records, item types,
//!   re-encode identity, the library root).
//!
//! Left to the viewer test, and counted: the `cache()` directory listings
//! (`*.cache_call`: `cache()`'s folder selection and gzip stay C++), the two
//! random worlds whose bytes are not committed (no input here), and the
//! lines of the realistic and Library cases that record the C++ model
//! (the world and populate summaries, the login journal and model digests,
//! the later login steps; see `r2_c2_load.rs` for what of a login the crate
//! replays).

mod common;
mod r2common;

use r2common::fixture_model::{HCat, HarnessModel};
use r2common::gz::gunzip_strict;
use r2common::snapshot::{agent, f, lf, lib_owner};
use r2common::*;
use vkstorm_inventory::cache::{self, Decoded};
use vkstorm_inventory::category::ViewerCategory;
use vkstorm_inventory::login::{CacheInput, Skeleton, SkeletonEntry};
use vkstorm_inventory::model::EmptyModel;
use vkstorm_inventory::types::FolderType;
use vkstorm_inventory::validate;
use vkstorm_llsd::{Format, Limits};

use common::sha256_hex;

fn decode(bytes: &[u8]) -> Decoded {
    cache::decode(bytes, false, &EmptyModel)
}

fn records(d: &Decoded) -> Vec<String> {
    let mut out: Vec<String> = d.categories.iter().map(|c| dump_category(c, -1)).collect();
    out.extend(d.items.iter().map(dump_item));
    out
}

fn decode_line(d: &Decoded, with_obsolete: bool) -> String {
    format!(
        "decode rv={}{} cats={} items={} cats_to_update={}",
        u8::from(d.rv()),
        if with_obsolete { format!(" obsolete={}", u8::from(d.obsolete)) } else { String::new() },
        d.categories.len(),
        d.items.len(),
        ids(&d.cats_to_update)
    )
}

fn reencode(d: &Decoded) -> Vec<u8> {
    cache::encode(&d.categories, &d.items)
}

fn reencode_line(again: &[u8], plain: &[u8], with_rv: bool) -> String {
    let identical = again == plain;
    let first = again.iter().zip(plain).take_while(|(a, b)| a == b).count();
    format!(
        "reencode{} identical={}{} size={}",
        if with_rv { " rv=1" } else { "" },
        u8::from(identical),
        if identical { String::new() } else { format!(" first_difference={first}") },
        again.len()
    )
}

/// The notation rendering the recorder writes beside a committed file.
fn notation_file(plain: &[u8]) -> String {
    let (_, body) = vkstorm_llsd::parse(Format::Binary, &plain[4..], Limits::UNLIMITED);
    llsd_notation(&body.unwrap_or(vkstorm_llsd::Value::Undefined)) + "\n"
}

#[test]
fn c1_cache_files() {
    let file = &cache_lib("c1_cache_files.txt");
    let mut tally = Tally::default();
    let library = read_bytes(&cache_lib("c1/standard.library.inv.llsd"));
    let library_digest = sha256_hex(&reencode(&decode(&library)));
    let mut notation_files = 0;
    for case in read_cases(file) {
        if case.id.ends_with(".cache_call") {
            tally.left("cache_call (cache() listing)", case.lines.len());
            continue;
        }
        if case.lines[0].starts_with("file (not committed)") {
            tally.left("uncommitted random world", case.lines.len());
            continue;
        }
        // The record names the file as the recorder's tree did (`cache/c1/`);
        // this build's copy is the one of its standard library.
        let label = format!("cache/c1/{}.inv.llsd", case.id);
        let plain = read_bytes(&cache_lib(&format!("c1/{}.inv.llsd", case.id)));
        let mut out = vec![format!("file {label} size={} sha256={}", plain.len(), sha256_hex(&plain))];
        let version = u32::from_be_bytes([plain[0], plain[1], plain[2], plain[3]]);
        out.push(format!("header={version} last_byte={}", plain.last().map(|&b| i32::from(b)).unwrap_or(-1)));
        let d = decode(&plain);
        out.push(decode_line(&d, true));
        out.extend(records(&d).into_iter().map(|l| format!("  {l}")));
        out.push(reencode_line(&reencode(&d), &plain, true));
        // The library's cache file follows the record order of its world's
        // model, which is unordered_map order since upstream 28fee038e0 and
        // so differs between worlds. Only the standard world's library file
        // is committed: the other worlds' line is left to the viewer test.
        let mut expected: &[String] = &case.lines;
        if case.lines.last().is_some_and(|l| l.starts_with("library file")) {
            if case.id == "standard.agent" {
                out.push(format!("library file size={} sha256={library_digest}", library.len()));
            } else {
                expected = &case.lines[..case.lines.len() - 1];
                tally.left("library file of a world whose library cache is not committed", 1);
            }
        }
        assert_case(file, &case.id, expected, &out);
        tally.replayed("cache file", out.len());

        let notation = String::from_utf8(read_bytes(&cache_lib(&format!("c1/{}.notation.txt", case.id)))).unwrap();
        assert_eq!(notation_file(&plain), notation, "{}.notation.txt", case.id);
        notation_files += 1;
    }
    tally.print(file);
    assert_eq!(notation_files, 6);
    // 4 library file lines of the non-standard worlds are left (see above).
    assert_eq!((tally.replayed, tally.left), (788, 30));
}

#[test]
fn c1_realistic() {
    let file = &cache_lib("c1_realistic.txt");
    let cases = read_cases(file);
    assert_eq!(cases.len(), 1);
    let case = &cases[0];
    let plain = gunzip_strict(&read_bytes(&cache_lib("c1/realistic_100k.inv.llsd.gz")));
    let d = decode(&plain);
    let recs = records(&d);
    let mut produced = vec![format!("file size={} sha256={}", plain.len(), sha256_hex(&plain))];
    produced.push(format!("{} records_sha256={}", decode_line(&d, false), digest_lines(&recs)));
    produced.extend(chunked_digests("records", &recs, 10000));
    produced.push(format!("reencode identical={}", u8::from(reencode(&d) == plain)));
    // The produced lines are the fixture's lines 2.. up to the validation
    // of the populated model.
    let start = case.lines.iter().position(|l| l.starts_with("file size=")).unwrap();
    assert_case(file, &case.id, &case.lines[start..start + produced.len()], &produced);
    let mut tally = Tally::default();
    tally.replayed("decode and re-encode", produced.len());

    // validate() of the populated model, rebuilt from the cache it wrote
    // (as r2_c4_validation.rs rebuilds populate): every folder has the
    // server's version and the count of its loaded children; the library
    // is the standard world's, without items.
    let mut populated = HarnessModel { root: f(1), library_root: lf(1), library_owner: lib_owner(), ..HarnessModel::default() };
    for c in d.categories.iter().cloned().chain(standard_library()) {
        populated.cats.insert(c.category.object.id, HCat { cat: c, count: 0 });
    }
    for it in &d.items {
        if it.is_link_type() {
            populated.backlinks.push((it.linked_id(), it.object.id));
        }
        populated.items.insert(it.object.id, it.clone());
    }
    let mut snap = populated.built_snapshot();
    let counts: Vec<i32> =
        snap.categories.iter().map(|c| (snap.category_tree[&c.id].len() + snap.item_tree[&c.id].len()) as i32).collect();
    for (c, n) in snap.categories.iter_mut().zip(counts) {
        c.descendent_count = n;
    }
    let v_start = start + produced.len();
    let validation = dump_validation(&validate::validate(&key_ordered(&snap), Default::default()).info);
    assert_case(file, "realistic_100k (populated)", &case.lines[v_start..v_start + validation.len()], &validation);
    tally.replayed("validation of the populated model (rebuilt)", validation.len());

    // The reload: a login from that cache with the world's skeletons (the
    // agent's: the cached folders with their versions; the standard
    // library's, without a cache), then validate() after
    // buildParentChildMap, rebuilt as it files a model where nothing is
    // lost.
    let entry = |c: &ViewerCategory| SkeletonEntry {
        name: Some(c.category.object.name.clone()),
        folder_id: Some(c.category.object.id),
        parent_id: Some(c.category.object.parent),
        version: Some(c.version),
        type_default: Some(c.category.preferred_type.0),
    };
    let lib_entries: Vec<SkeletonEntry> = standard_library().iter().map(entry).collect();
    let agent_entries: Vec<SkeletonEntry> = d.categories.iter().map(entry).collect();
    let mut model = HarnessModel { root: f(1), library_root: lf(1), library_owner: lib_owner(), ..HarnessModel::default() };
    let lib_plan = Skeleton::import(&lib_owner(), &lib_entries).load(&CacheInput::default(), false, &model);
    let lib_rv = lib_plan.rv;
    model.apply(lib_plan);
    let input = CacheInput { gz_present: true, gunzip_ok: true, file: Some(&plain), second_instance: false };
    let agent_plan = Skeleton::import(&agent(), &agent_entries).load(&input, false, &model);
    let agent_rv = agent_plan.rv;
    model.apply(agent_plan);
    let known = model.cats.values().filter(|c| c.cat.version != -1).count();
    let links = model.items.values().filter(|i| i.is_link_type()).count();
    let summary =
        format!("reload cats={} cats_known_version={known} items={} links={links}", model.cats.len(), model.items.len());
    let at = case.lines.iter().position(|l| l.starts_with("reload cats=")).unwrap();
    assert_case(file, "realistic_100k (reload summary)", &case.lines[at..=at], &[summary]);
    let steps = [format!("step load_library rv={}", u8::from(lib_rv)), format!("step load_agent rv={}", u8::from(agent_rv))];
    let at = case.lines.iter().position(|l| l.starts_with("step load_library")).unwrap();
    assert_case(file, "realistic_100k (reload steps)", &case.lines[at..at + 2], &steps);
    let mut info = validate::validate(&key_ordered(&model.built_snapshot()), Default::default()).info;
    info.initialized = true;
    let reload_validation = dump_validation(&info);
    let at = at + 3; // after "step build_parent_child_map"
    assert_case(file, "realistic_100k (reload validation)", &case.lines[at..at + reload_validation.len()], &reload_validation);
    tally.replayed("reload: summary, load steps, validation", 1 + steps.len() + reload_validation.len());
    tally.left("world, populated summary, reload digests, build and later steps", case.lines.len() - tally.replayed);
    tally.print(file);
    assert_eq!((tally.replayed, tally.left), (25, 20));
}

/// The standard world's library folders (`standard_world()`), with the
/// server's versions.
fn standard_library() -> Vec<ViewerCategory> {
    let mk = |id, parent, ft: i32, name: &[u8], version| {
        let mut c = ViewerCategory { owner: lib_owner(), version, ..ViewerCategory::default() };
        c.category.object.id = id;
        c.category.object.parent = parent;
        c.category.object.name = name.to_vec();
        c.category.preferred_type = FolderType(ft);
        c
    };
    vec![
        mk(lf(1), [0; 16], 8, b"Library", 11),
        mk(lf(2), lf(1), 5, b"Clothing", 5),
        mk(lf(3), lf(1), 6, b"Objects", 4),
        mk(lf(4), lf(3), -1, b"Furniture", 2),
    ]
}

#[test]
fn library_real() {
    let file = &cache_lib("library_real.txt");
    let cases = read_cases(file);
    let case = &cases[0];
    let name = "ba2a564a-f0f1-4b82-9c61-b7520bfcd09f.inv.llsd.gz";
    let plain = gunzip_strict(&read_bytes(&format!("model/library/{name}")));
    let d = decode(&plain);
    let recs = records(&d);
    let mut out = vec![format!("source {name} plain_size={} plain_sha256={}", plain.len(), sha256_hex(&plain))];
    out.push(format!("{} records_sha256={}", decode_line(&d, true), digest_lines(&recs)));
    out.extend(chunked_digests("records", &recs, 1000));
    let mut types = std::collections::BTreeMap::<i32, u32>::new();
    for it in &d.items {
        *types.entry(it.object.asset_type.0).or_default() += 1;
    }
    let mut type_line = String::from("item_types");
    for (t, n) in &types {
        type_line += &format!(" {t}:{n}");
    }
    out.push(type_line);
    out.push(reencode_line(&reencode(&d), &plain, false));
    let root = d.categories.iter().filter(|c| c.category.object.parent == [0; 16]).last().unwrap();
    out.push(format!("library_root {}", uuid(&root.category.object.id)));
    assert_case(file, &case.id, &case.lines[..out.len()], &out);
    let mut tally = Tally::default();
    tally.replayed("decode and re-encode", out.len());
    // The login as a library skeleton: r2_c2_load.rs replays its file
    // listings; the model summary and digests and the validation record
    // the C++ model after buildParentChildMap.
    tally.left("login model, digests, validation", case.lines.len() - out.len());
    tally.print(file);
    assert_eq!((tally.replayed, tally.left), (12, 18));
}
