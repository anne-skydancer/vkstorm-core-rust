//! C5 items, categories and objects: regenerates every line of
//! `fixtures/model/c5_items.txt`, `c5_matrix.txt`, `c5_categories.txt` and the
//! `c5r` random-stream sample from the crate, as `c5_item_record()`,
//! `c5_matrix()`, `c5_cat_record()` and `c5_categories()` in
//! `llinventory_tool.cpp` write them from the frozen C++, and checks the `c5r`
//! and `c5mf` streams against their chunked SHA-256 digests.
//!
//! Each replay also prints its lines by kind: the records (constructors,
//! copies, setters, derived getters, slam bits), the CRCs, the legacy text
//! forms, the LLSD forms and the matrix, with the lines that carry a
//! `shadow_id` counted again on their own.

mod common;

use std::collections::BTreeMap;
use std::time::Instant;

use common::*;
use vkstorm_inventory::category::Category;
use vkstorm_inventory::item::Item;
use vkstorm_inventory::legacy::reader::Stream;
use vkstorm_inventory::llsd::coerce::{as_integer, as_string, get};
use vkstorm_inventory::object::Object;
use vkstorm_inventory::types::{AssetType, FolderType, InventoryType};
use vkstorm_llsd::Value;


/// The asset line of an item's legacy text and its byte range
/// (`\t\tasset_id\t` or else `\t\tshadow_id\t` up to its newline).
fn asset_line(t: &[u8]) -> Option<(usize, usize)> {
    let find = |pat: &[u8]| t.windows(pat.len()).position(|w| w == pat);
    let a = find(b"\t\tasset_id\t").or_else(|| find(b"\t\tshadow_id\t"))?;
    let b = a + t[a..].iter().position(|&c| c == b'\n')? + 1;
    Some((a, b))
}

/// `c5_item_record<I>()` over the crate.
fn c5_item_record(cs: &str, f: &ItemF, s: &mut Lines) {
    let item = make_item(f);
    s.emit(cs, "in", &item_cols(&item));
    let sale_ok = (0..=3).contains(&f.sale_type);
    let mut key = Vec::new();
    let r1 = item.export_legacy_stream(&mut key, true);
    let mut nokey = Vec::new();
    let r2 = item.export_legacy_stream(&mut nokey, false);
    s.emit(cs, "legacy", &[esc(&key)]);
    if sale_ok {
        let (k, n) = (asset_line(&key), asset_line(&nokey));
        let rest_same = match (k, n) {
            (Some((ka, kb)), Some((na, nb))) => key[..ka] == nokey[..na] && key[kb..] == nokey[nb..],
            _ => false,
        };
        let nv = n.map_or_else(|| "-".to_owned(), |(na, nb)| esc(&nokey[na..nb]));
        s.emit(cs, "legacy.nokey", &[nv, bl(rest_same)]);
        s.emit(cs, "legacy.ret", &[bl(r1), bl(r2)]);
    } else {
        // Written as "not" since bug 1: the stream stays good both ways.
        s.emit(cs, "legacy.outcome", &["good", "good"]);
        s.emit(cs, "legacy.nokey.prefix", &[esc(&nokey)]);
    }
    {
        let text = after_first_line(&key);
        let mut is = Stream::new(text);
        let mut back = make_item(&prev_item());
        let ok = back.import_legacy_stream(&mut is);
        s.emit(cs, "legacy.reimport", &item_cols(&back));
        s.emit(cs, "legacy.reimport.stream", &with_ret(ok, stream_cols(&is, text)));
    }
    {
        let mut obj = Vec::new();
        item.object.export_legacy_stream(&mut obj);
        s.emit(cs, "legacy.object", &[esc(&obj)]);
    }
    let sd = item.to_llsd();
    s.emit_llsd(cs, "llsd", &sd);
    {
        let mut into = Value::map();
        if let Value::Map(m) = &mut into {
            m.insert(b"preexisting".to_vec(), Value::Integer(1));
            m.insert(b"name".to_vec(), Value::string("overwritten"));
        }
        item.fill_llsd(&mut into);
        let Value::Map(m) = &into else { unreachable!() };
        let keys: Vec<&str> = m.keys().map(|k| std::str::from_utf8(k).unwrap()).collect();
        let same = m
            .iter()
            .filter(|(k, _)| k.as_slice() != b"preexisting")
            .all(|(k, v)| llsd_bin(v) == llsd_bin(get(&sd, std::str::from_utf8(k).unwrap())));
        s.emit(cs, "llsd.into", &[esc_str(&keys.join(",")), bl(same), d(as_integer(get(&into, "preexisting")))]);
    }
    s.emit(cs, "crc", &[x8(item.crc32())]);
    s.emit(
        cs,
        "derived",
        &[
            uuid(&item.linked_id()),
            bl(item.is_link_type()),
            d(item.inventory_sub_type()),
            uuid(&item.creator()),
            d(item.asset_type().0),
        ],
    );
    {
        let mut n = Item::default();
        let ok = n.from_llsd(&sd, true, false);
        s.emit(cs, "fromllsd.new", &item_cols(&n));
        s.emit(cs, "fromllsd.new.ret", &[bl(ok), bl(llsd_bin(&n.to_llsd()) == llsd_bin(&sd))]);
    }
    {
        let mut o = make_item(&prev_item());
        o.from_llsd(&sd, false, false);
        s.emit(cs, "fromllsd.reuse", &item_cols(&o));
    }
    {
        let mut o = make_item(&prev_item());
        o.from_llsd(&sd, true, true);
        s.emit(cs, "fromllsd.opensim", &item_cols(&o));
    }
    s.emit(cs, "sdcreate.bin", &[llsd_bin(&item.create_sd())]);
    {
        let mut x = make_item(f);
        x.accumulate_permission_slam_bits(&make_item(&prev_item()));
        let mut y = make_item(&prev_item());
        y.accumulate_permission_slam_bits(&item);
        s.emit(cs, "slam", &[x8(x.flags), x8(y.flags)]);
    }
    {
        let mut x = make_item(f);
        x.set_permissions(&make_perm(&prev_item().perm));
        s.emit(cs, "setpermissions", &perm_cols(&x.permissions));
        s.emit(cs, "copy", &item_cols(&Item::copy_from(&item)));
        let mut z = make_item(f);
        z.set_inventory_sub_type(0x7e);
        s.emit(cs, "subtype", &[x8(z.flags), d(z.inventory_sub_type())]);
        let mut w = make_item(&prev_item());
        w.copy_item(&item);
        s.emit(cs, "copyitem", &item_cols(&w));
        let k = Item::new(
            &f.id,
            &f.parent,
            &make_perm(&f.perm),
            &f.asset,
            AssetType(f.ty),
            InventoryType(f.inv_type),
            &f.name,
            &f.desc,
            &make_sale(f.sale_type, f.sale_price),
            f.flags,
            f.date as i32,
        );
        s.emit(cs, "ctor", &item_cols(&k));
    }
}

fn c5_matrix_lines() -> Lines {
    let mut s = Lines::default();
    for at in ASSET_TYPES {
        for it in INV_TYPES {
            let f = ItemF { ty: at, inv_type: it, ..base_item() };
            let item = make_item(&f);
            let mut text = Vec::new();
            item.export_legacy_stream(&mut text, true);
            let token = |key: &str| -> String {
                let pat = format!("\t\t{key}\t");
                match text.windows(pat.len()).position(|w| w == pat.as_bytes()) {
                    None => "-".to_owned(),
                    Some(p) => {
                        let a = p + pat.len();
                        let e = a + text[a..].iter().position(|&c| c == b'\n').unwrap_or(text.len() - a);
                        esc(&text[a..e])
                    }
                }
            };
            let body = after_first_line(&text);
            let mut back = make_item(&prev_item());
            back.import_legacy_stream(&mut Stream::new(body));
            let sd = item.to_llsd();
            let sit = get(&sd, "inv_type");
            let mut n = Item::default();
            n.from_llsd(&sd, true, false);
            let sit_col = match sit {
                Value::String(v) => format!("S:{}", esc(v)),
                _ => format!("I:{}", as_integer(sit)),
            };
            s.emit(
                &format!("c5m.{at}.{it}"),
                "matrix",
                &[
                    token("type"),
                    token("inv_type"),
                    esc(&as_string(get(&sd, "type"))),
                    sit_col,
                    x8(item.crc32()),
                    d(back.object.asset_type.0),
                    d(back.inv_type.0),
                    x8(back.permissions.base),
                    d(n.object.asset_type.0),
                    d(n.inv_type.0),
                    x8(n.permissions.base),
                    x8(n.permissions.everyone),
                    uuid(&item.linked_id()),
                ],
            );
        }
    }
    s
}


/// `c5_cat_record<I>()` over the crate.
fn c5_cat_record(cs: &str, f: &CatF, s: &mut Lines) {
    let cat = make_cat(f);
    s.emit(cs, "in", &cat_cols(&cat));
    {
        let mut os = Vec::new();
        cat.export_legacy_stream(&mut os);
        s.emit(cs, "legacy", &[esc(&os)]);
        let text = after_first_line(&os);
        let mut is = Stream::new(text);
        let mut back = make_cat(&prev_cat());
        let ok = back.import_legacy_stream(&mut is);
        s.emit(cs, "legacy.reimport", &cat_cols(&back));
        s.emit(cs, "legacy.reimport.stream", &with_ret(ok, stream_cols(&is, text)));
    }
    {
        let mut os = Vec::new();
        cat.object.export_legacy_stream(&mut os);
        s.emit(cs, "legacy.object", &[esc(&os)]);
        let text = after_first_line(&os);
        let mut is = Stream::new(text);
        let mut back = make_obj(&prev_cat());
        let ok = back.import_legacy_stream(&mut is);
        s.emit(cs, "legacy.object.reimport", &obj_cols(&back));
        s.emit(cs, "legacy.object.reimport.stream", &with_ret(ok, stream_cols(&is, text)));
    }
    let as_sd = cat.to_llsd();
    let ais = cat.to_ais_create_llsd();
    let mut ex = Value::Undefined;
    cat.export_llsd(&mut ex);
    let vcat = make_vcat(f);
    let mut vex = Value::Undefined;
    vcat.export_llsd(&mut vex);
    s.emit_llsd(cs, "asllsd", &as_sd);
    s.emit_llsd(cs, "ais", &ais);
    s.emit_llsd(cs, "export", &ex);
    s.emit_llsd(cs, "vexport", &vex);
    {
        let mut c = make_cat(&prev_cat());
        let ok = c.from_llsd(&as_sd);
        let mut cols = cat_cols(&c);
        cols.push(bl(ok));
        s.emit(cs, "fromllsd.asllsd", &cols);
        let mut c2 = make_cat(&prev_cat());
        c2.from_llsd(&ais);
        s.emit(cs, "fromllsd.ais", &cat_cols(&c2));
    }
    {
        let mut c = Category::default();
        let ok = c.import_llsd_map(&ex);
        let mut cols = cat_cols(&c);
        cols.push(bl(ok));
        s.emit(cs, "importmap", &cols);
        let mut v = make_vcat(&CatF::default());
        v.import_llsd_map(&vex);
        s.emit(cs, "vimportmap", &vcat_cols(&v));
        let mut again = Value::Undefined;
        v.export_llsd(&mut again);
        s.emit(cs, "vimportmap.identity", &[bl(llsd_bin(&again) == llsd_bin(&vex))]);
    }
    {
        let c = cat.create_sd();
        s.emit(cs, "sdcreate.bin", &[llsd_bin(&c)]);
        s.emit(cs, "sdcreate.back", &cat_cols(&Category::from_create_sd(&c)));
        s.emit(cs, "copy", &cat_cols(&Category::copy_from(&cat)));
        let ctor = Category::new(&f.id, &f.parent, FolderType(f.pref), &f.name);
        s.emit(cs, "ctor", &cat_cols(&ctor));
    }
}

/// `c5_categories<I>()` over the crate: the categories, then the objects.
fn c5_categories_lines() -> Lines {
    let mut s = Lines::default();
    for (cs, f) in c5_cat_cases() {
        c5_cat_record(&cs, &f, &mut s);
    }
    let names: [Vec<u8>; 6] =
        [b"Object name".to_vec(), Vec::new(), b"  spaced  ".to_vec(), vec![b'o'; 80], b"pipe|obj".to_vec(), b"caf\xc3\xa9".to_vec()];
    for (i, name) in names.iter().enumerate() {
        let f = CatF {
            id: u(0xe5),
            parent: u(0xe6),
            ty: if i == 3 { 255 } else { 6 },
            name: name.clone(),
            thumb: if i == 2 { u(0xb3) } else { NUL },
            ..CatF::default()
        };
        let cs = format!("c5o.{i}");
        let o = make_obj(&f);
        s.emit(&cs, "in", &obj_cols(&o));
        let mut os = Vec::new();
        o.export_legacy_stream(&mut os);
        s.emit(&cs, "legacy", &[esc(&os)]);
        let text = after_first_line(&os);
        let mut is = Stream::new(text);
        let mut back = make_obj(&prev_cat());
        let ok = back.import_legacy_stream(&mut is);
        s.emit(&cs, "legacy.reimport", &obj_cols(&back));
        s.emit(&cs, "legacy.reimport.stream", &with_ret(ok, stream_cols(&is, text)));
        let ctor = Object::new(&f.id, &f.parent, AssetType(f.ty), &f.name);
        s.emit(&cs, "ctor", &obj_cols(&ctor));
        let mut cp = make_obj(&prev_cat());
        cp.copy_object(&o);
        s.emit(&cs, "copyobject", &obj_cols(&cp));
        let mut st2 = make_obj(&prev_cat());
        st2.set_type(AssetType(10));
        let rt1 = st2.runtime.clone();
        st2.set_type(AssetType(6));
        s.emit(&cs, "settype", &[esc(&rt1), esc(&st2.runtime)]);
    }
    s
}

// ---------------------------------------------------------------------------
// Random items (draw_item)
// ---------------------------------------------------------------------------

const C5R_SEED: u64 = 0x7635_0000_0000_0001;
const C5R_CASES: u64 = 20000;

fn draw_id(r: &mut Rng) -> [u8; 16] {
    match r.below(4) {
        0 => NUL,
        1 => id_a(),
        2 => id_b(),
        _ => id_g(),
    }
}

fn draw_mask(r: &mut Rng) -> u32 {
    let v = r.next();
    if v & 1 != 0 { MASK_EDGES[((v >> 1) % 18) as usize] } else { (v >> 32) as u32 }
}

fn draw_perm(r: &mut Rng) -> PermF {
    let creator = draw_id(r);
    let owner = draw_id(r);
    let last = draw_id(r);
    let group = draw_id(r);
    let base = draw_mask(r);
    let owner_mask = draw_mask(r);
    let group_mask = draw_mask(r);
    let everyone = draw_mask(r);
    let next = draw_mask(r);
    let group_owned = r.coin();
    PermF { creator, owner, last, group, base, owner_mask, group_mask, everyone, next, group_owned }
}

fn draw_price(r: &mut Rng) -> i32 {
    let v = r.next();
    if v & 1 != 0 {
        PRICES[((v >> 1) % 5) as usize]
    } else if v & 2 != 0 {
        ((v >> 8) % 100000) as i32
    } else {
        (v >> 32) as u32 as i32
    }
}

fn draw_item(r: &mut Rng, names: &[(Vec<u8>, Vec<u8>)]) -> ItemF {
    const RT_DATES: [i64; 6] = [0, 1, i32::MAX as i64, i32::MIN as i64, -1, 1700000000];
    let mut f = base_item();
    f.ty = ASSET_TYPES[r.below(36) as usize];
    f.inv_type = INV_TYPES[r.below(27) as usize];
    f.asset = match r.below(5) {
        0 => NUL,
        1 => u(0xa3),
        2 => MAGIC,
        3 => not_magic(),
        _ => {
            let a = r.next();
            let b = r.next();
            let mut id = [0u8; 16];
            id[..8].copy_from_slice(&a.to_le_bytes());
            id[8..].copy_from_slice(&b.to_le_bytes());
            id
        }
    };
    f.thumb = if r.coin() { NUL } else { u(0xb1) };
    f.fav = r.coin();
    f.runtime = match r.below(4) {
        0 => Vec::new(),
        1 => b"luau".to_vec(),
        2 => vec![b'r'; 60],
        _ => b"lu au".to_vec(),
    };
    f.flags = if r.coin() { FLAGS[r.below(7) as usize] } else { r.next() as u32 };
    f.perm = draw_perm(r);
    f.sale_type = r.below(4) as i32;
    f.sale_price = draw_price(r);
    f.name = names[r.below(16) as usize].0.clone();
    f.desc = names[r.below(16) as usize].1.clone();
    f.date = if r.coin() { RT_DATES[r.below(6) as usize] } else { i64::from(r.next() as u32 as i32) };
    f
}

// ---------------------------------------------------------------------------
// Reporting by kind of line
// ---------------------------------------------------------------------------

/// The kind of a line, by its field.
fn kind(field: &str) -> &'static str {
    const LLSD: [&str; 9] = ["llsd", "fromllsd", "sdcreate", "asllsd", "ais", "export", "vexport", "importmap", "vimportmap"];
    if field == "crc" {
        "crc"
    } else if field == "matrix" {
        "matrix"
    } else if field.starts_with("legacy") {
        "legacy"
    } else if LLSD.iter().any(|p| field == *p || field.starts_with(&format!("{p}."))) {
        "llsd"
    } else {
        "record"
    }
}

/// Prints matched/total per kind of line (exact equality; C5 holds no `UB`
/// or `RT(...)` value), with the lines carrying a `shadow_id` again.
fn report_kinds(fixture: &str, expected: &[String], actual: &[String]) {
    let mut by: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for (i, e) in expected.iter().enumerate() {
        let field = e.split('\t').nth(1).unwrap_or("");
        let ok = actual.get(i) == Some(e);
        let mut tally = |k| {
            let t = by.entry(k).or_insert((0, 0));
            t.1 += 1;
            if ok {
                t.0 += 1;
            }
        };
        tally(kind(field));
        if e.contains("shadow_id") || (field == "llsd.bin" && e.contains("736861646f775f6964")) {
            tally("(shadow_id)");
        }
    }
    let parts: Vec<String> = by.iter().map(|(k, (m, t))| format!("{k} {m}/{t}")).collect();
    println!("{fixture}: by kind: {}", parts.join(", "));
}

fn replay(fixture: &str, actual: &[String]) {
    let expected = read_fixture(fixture);
    report_kinds(fixture, &expected, actual);
    assert_replay(fixture, &expected, actual);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn c5_items() {
    let start = Instant::now();
    let mut s = Lines::default();
    for (cs, f) in c5_item_cases() {
        c5_item_record(&cs, &f, &mut s);
    }
    replay("c5_items.txt", &s.0);
    println!("c5_items: {:.1?}", start.elapsed());
}

#[test]
fn c5_matrix() {
    replay("c5_matrix.txt", &c5_matrix_lines().0);
}

#[test]
fn c5_categories() {
    replay("c5_categories.txt", &c5_categories_lines().0);
}

#[test]
fn c5_random_stream() {
    let start = Instant::now();
    let names = c5_names();
    let mut rng = Rng::new(C5R_SEED);
    let mut digest = ChunkedDigest::new("c5r", 2000);
    let mut sample = Vec::new();
    for i in 0..C5R_CASES {
        digest.begin_case(i);
        let f = draw_item(&mut rng, &names);
        let mut lines = Lines::default();
        c5_item_record(&format!("c5r.{i}"), &f, &mut lines);
        for line in &lines.0 {
            digest.line(line);
        }
        if i < 10 || i % 2000 == 0 {
            sample.extend(lines.0);
        }
    }
    let entries = digest.finish();
    replay("c5_random_sample.txt", &sample);
    assert_digests("c5r", &entries);
    println!("c5r: {C5R_CASES} cases in {:.1?}", start.elapsed());
}

#[test]
fn c5_matrix_full_records() {
    let start = Instant::now();
    let n = INV_TYPES.len() as u64;
    let mut digest = ChunkedDigest::new("c5mf", n);
    for i in 0..(ASSET_TYPES.len() as u64 * n) {
        digest.begin_case(i);
        let f = ItemF { ty: ASSET_TYPES[(i / n) as usize], inv_type: INV_TYPES[(i % n) as usize], ..base_item() };
        let mut lines = Lines::default();
        c5_item_record(&format!("c5mf.{}.{}", f.ty, f.inv_type), &f, &mut lines);
        for line in &lines.0 {
            digest.line(line);
        }
    }
    assert_digests("c5mf", &digest.finish());
    println!("c5mf: {:.1?}", start.elapsed());
}
