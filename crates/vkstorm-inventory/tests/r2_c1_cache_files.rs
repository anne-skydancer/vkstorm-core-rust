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
//!   decode digests per 10,000 records, re-encode identity);
//! - the Library cache (decode digests per 1,000 records, item types,
//!   re-encode identity, the library root).
//!
//! Left to the viewer test, and counted: the `cache()` directory listings
//! (`*.cache_call`: `cache()`'s folder selection and gzip stay C++), the two
//! random worlds whose bytes are not committed (no input here), and the
//! lines of the realistic and Library cases that record the C++ model
//! (populate, login, model digests; see `r2_c2_load.rs` for what of a login
//! the crate replays).

mod common;
mod r2common;

use r2common::gz::gunzip_strict;
use r2common::*;
use vkstorm_inventory::cache::{self, Decoded};
use vkstorm_inventory::model::EmptyModel;
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
    let file = "cache/c1_cache_files.txt";
    let mut tally = Tally::default();
    let library = read_bytes("cache/c1/standard.library.inv.llsd");
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
        let rel = format!("cache/c1/{}.inv.llsd", case.id);
        let plain = read_bytes(&rel);
        let mut out = vec![format!("file {rel} size={} sha256={}", plain.len(), sha256_hex(&plain))];
        let version = u32::from_be_bytes([plain[0], plain[1], plain[2], plain[3]]);
        out.push(format!("header={version} last_byte={}", plain.last().map(|&b| i32::from(b)).unwrap_or(-1)));
        let d = decode(&plain);
        out.push(decode_line(&d, true));
        out.extend(records(&d).into_iter().map(|l| format!("  {l}")));
        out.push(reencode_line(&reencode(&d), &plain, true));
        if case.lines.last().is_some_and(|l| l.starts_with("library file")) {
            out.push(format!("library file size={} sha256={library_digest}", library.len()));
        }
        assert_case(file, &case.id, &case.lines, &out);
        tally.replayed("cache file", out.len());

        let notation = String::from_utf8(read_bytes(&format!("cache/c1/{}.notation.txt", case.id))).unwrap();
        assert_eq!(notation_file(&plain), notation, "{}.notation.txt", case.id);
        notation_files += 1;
    }
    tally.print(file);
    assert_eq!(notation_files, 6);
    assert_eq!((tally.replayed, tally.left), (792, 26));
}

#[test]
fn c1_realistic() {
    let file = "cache/c1_realistic.txt";
    let cases = read_cases(file);
    assert_eq!(cases.len(), 1);
    let case = &cases[0];
    let plain = gunzip_strict(&read_bytes("cache/c1/realistic_100k.inv.llsd.gz"));
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
    tally.left("world, populated model, validation, reload", case.lines.len() - produced.len());
    tally.print(file);
    assert_eq!((tally.replayed, tally.left), (14, 31));
}

#[test]
fn library_real() {
    let file = "cache/library_real.txt";
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
