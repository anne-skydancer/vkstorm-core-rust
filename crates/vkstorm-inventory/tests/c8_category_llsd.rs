//! C8 category LLSD input: regenerates every line of
//! `fixtures/model/c8_category_llsd.txt` from the crate, as
//! `c8_category_llsd()` in `llinventory_tool.cpp` writes it from the frozen
//! C++ (`LLInventoryCategory::fromLLSD`, `importLLSD`/`importLLSDMap` and the
//! viewer subclass's cache keys over 13 keys x 48 values, the thumbnail and
//! type pairs, and one object reused across records).
#![allow(dead_code)]

mod common;

use common::*;
use vkstorm_inventory::category::{Category, ViewerCategory};
use vkstorm_inventory::item::Item;
use vkstorm_inventory::llsd::coerce::insert;
use vkstorm_inventory::object::Object;
use vkstorm_inventory::perms::{Permissions, PERM_ALL};
use vkstorm_inventory::sale::{SaleInfo, SaleType};
use vkstorm_inventory::types::{AssetType, FolderType, InventoryType};
use vkstorm_llsd::text::date_parse;
use vkstorm_llsd::Value;

// ---------------------------------------------------------------------------
// Field records (the tool's PermF, ItemF, CatF) and raw construction
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct PermF {
    creator: [u8; 16],
    owner: [u8; 16],
    last: [u8; 16],
    group: [u8; 16],
    base: u32,
    owner_mask: u32,
    group_mask: u32,
    everyone: u32,
    next: u32,
    group_owned: bool,
}

impl Default for PermF {
    fn default() -> Self {
        PermF {
            creator: NUL,
            owner: NUL,
            last: NUL,
            group: NUL,
            base: PERM_ALL,
            owner_mask: PERM_ALL,
            group_mask: PERM_ALL,
            everyone: PERM_ALL,
            next: PERM_ALL,
            group_owned: false,
        }
    }
}

#[derive(Clone)]
struct ItemF {
    id: [u8; 16],
    parent: [u8; 16],
    asset: [u8; 16],
    thumb: [u8; 16],
    ty: i32,
    inv_type: i32,
    flags: u32,
    perm: PermF,
    sale_type: i32,
    sale_price: i32,
    date: i64,
    fav: bool,
    runtime: Vec<u8>,
    name: Vec<u8>,
    desc: Vec<u8>,
}

impl Default for ItemF {
    fn default() -> Self {
        ItemF {
            id: NUL,
            parent: NUL,
            asset: NUL,
            thumb: NUL,
            ty: -1,
            inv_type: -1,
            flags: 0,
            perm: PermF::default(),
            sale_type: 0,
            sale_price: 10,
            date: 0,
            fav: false,
            runtime: Vec::new(),
            name: Vec::new(),
            desc: Vec::new(),
        }
    }
}

#[derive(Clone)]
struct CatF {
    id: [u8; 16],
    parent: [u8; 16],
    thumb: [u8; 16],
    ty: i32,
    pref: i32,
    fav: bool,
    date: i64,
    runtime: Vec<u8>,
    name: Vec<u8>,
    owner: [u8; 16],
    version: i32,
}

impl Default for CatF {
    fn default() -> Self {
        CatF {
            id: NUL,
            parent: NUL,
            thumb: NUL,
            ty: 8,
            pref: -1,
            fav: false,
            date: 0,
            runtime: Vec::new(),
            name: Vec::new(),
            owner: NUL,
            version: -1,
        }
    }
}

/// `make_perm`: `init`, the five raw masks, then `yesReallySetOwner`.
fn make_perm(f: &PermF) -> Permissions {
    let mut p = Permissions::default();
    p.init(&f.creator, &f.owner, &f.last, &f.group);
    p.base = f.base;
    p.owner_mask = f.owner_mask;
    p.group_mask = f.group_mask;
    p.everyone = f.everyone;
    p.next = f.next;
    p.yes_really_set_owner(&f.owner, f.group_owned);
    p
}

fn make_sale(t: i32, p: i32) -> SaleInfo {
    SaleInfo { sale_type: SaleType(t), price: p }
}

/// `make_item` (`RawItem::assign`): every field raw.
fn make_item(f: &ItemF) -> Item {
    Item {
        object: Object {
            id: f.id,
            parent: f.parent,
            thumbnail: f.thumb,
            asset_type: AssetType(f.ty),
            favorite: f.fav,
            creation_date: f.date,
            runtime: f.runtime.clone(),
            name: f.name.clone(),
        },
        permissions: make_perm(&f.perm),
        asset: f.asset,
        description: f.desc.clone(),
        sale_info: make_sale(f.sale_type, f.sale_price),
        inv_type: InventoryType(f.inv_type),
        flags: f.flags,
    }
}

/// `make_obj` (`RawObj::assign`).
fn make_obj(f: &CatF) -> Object {
    Object {
        id: f.id,
        parent: f.parent,
        thumbnail: f.thumb,
        asset_type: AssetType(f.ty),
        favorite: f.fav,
        creation_date: f.date,
        runtime: f.runtime.clone(),
        name: f.name.clone(),
    }
}

/// `make_cat` (`RawCat::assign`).
fn make_cat(f: &CatF) -> Category {
    Category { object: make_obj(f), preferred_type: FolderType(f.pref) }
}

/// `make_vcat`: `make_cat` plus the owner and version.
fn make_vcat(f: &CatF) -> ViewerCategory {
    ViewerCategory { category: make_cat(f), owner: f.owner, version: f.version }
}

// ---------------------------------------------------------------------------
// Columns (the tool's item_cols, cat_cols, vcat_cols, obj_cols)
// ---------------------------------------------------------------------------

fn item_cols(it: &Item) -> Vec<String> {
    let o = &it.object;
    let mut v = vec![uuid(&o.id), uuid(&o.parent), uuid(&it.asset), uuid(&o.thumbnail), d(o.asset_type.0), d(it.inv_type.0), x8(it.flags)];
    let p = perm_cols(&it.permissions);
    v.extend(p);
    v.extend([
        d(it.sale_info.sale_type.0),
        d(it.sale_info.price),
        d(o.creation_date),
        bl(o.favorite),
        esc(&o.runtime),
        esc(&o.name),
        esc(&it.description),
    ]);
    v
}

fn cat_cols(c: &Category) -> Vec<String> {
    let o = &c.object;
    vec![
        uuid(&o.id),
        uuid(&o.parent),
        d(o.asset_type.0),
        d(c.preferred_type.0),
        uuid(&o.thumbnail),
        bl(o.favorite),
        d(o.creation_date),
        esc(&o.runtime),
        esc(&o.name),
    ]
}

fn vcat_cols(c: &ViewerCategory) -> Vec<String> {
    let mut v = cat_cols(&c.category);
    v.push(uuid(&c.owner));
    v.push(d(c.version));
    v
}

fn obj_cols(o: &Object) -> Vec<String> {
    vec![
        uuid(&o.id),
        uuid(&o.parent),
        d(o.asset_type.0),
        uuid(&o.thumbnail),
        bl(o.favorite),
        d(o.creation_date),
        esc(&o.runtime),
        esc(&o.name),
    ]
}

fn with_ret(ret: bool, mut cols: Vec<String>) -> Vec<String> {
    cols.insert(0, bl(ret));
    cols
}

// ---------------------------------------------------------------------------
// The cases (the tool's base_item, prev_item, c5_names, c5_item_cases, ...)
// ---------------------------------------------------------------------------

const ASSET_TYPES: [i32; 36] =
    [-1, 0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 13, 17, 18, 19, 20, 21, 22, 24, 25, 40, 45, 49, 56, 57, 58, 59, 255, 26, 60, 127, -2, 50, 9];
const INV_TYPES: [i32; 27] = [0, 1, 2, 3, 6, 7, 8, 9, 10, 15, 17, 18, 19, 20, 22, 23, 24, 25, 26, 27, 28, -1, 255, 4, 5, 11, 29];
const BASE_MASKS: [u32; 7] = [PERM_ALL, 0x7fffbfff, 0x7fff7fff, 0x7fffdfff, 0x7fff5fff, 0x00000000, 0x0000e000];
const FLAGS: [u32; 7] = [0, 1, 0xff, 0x100, 0x1f1100, 0x7fffffff, 0xffffffff];
const DATES: [i64; 7] = [0, 1, i32::MAX as i64, i32::MIN as i64, 2147483648, 4294967301, -1];
const PRICES: [i32; 5] = [i32::MIN, -1, 0, 10, i32::MAX];
/// `T_ | V_`: transfer and move.
const NEXT_DEFAULT: u32 = 0x0008_2000;

fn base_item() -> ItemF {
    ItemF {
        id: u(0xa1),
        parent: u(0xa2),
        asset: u(0xa3),
        ty: 6,
        inv_type: 6,
        flags: 0,
        perm: PermF {
            creator: id_c(),
            owner: id_a(),
            last: id_b(),
            group: id_g(),
            base: PERM_ALL,
            owner_mask: PERM_ALL,
            group_mask: 0,
            everyone: 0,
            next: NEXT_DEFAULT,
            group_owned: false,
        },
        sale_type: 0,
        sale_price: 10,
        date: 1700000000,
        name: b"Base item".to_vec(),
        desc: b"Base description".to_vec(),
        ..ItemF::default()
    }
}

fn prev_item() -> ItemF {
    ItemF {
        id: u(0x11),
        parent: u(0x12),
        asset: u(0x13),
        thumb: u(0x14),
        ty: 7,
        inv_type: 7,
        flags: 0x101,
        perm: PermF {
            creator: u(0x21),
            owner: u(0x22),
            last: u(0x23),
            group: u(0x24),
            base: PERM_ALL,
            owner_mask: 0x0008e000,
            group_mask: 0x00002000,
            everyone: 0,
            next: 0x00082000,
            group_owned: false,
        },
        sale_type: 2,
        sale_price: 25,
        date: 1234567890,
        fav: true,
        runtime: b"pre-runtime".to_vec(),
        name: b"pre name".to_vec(),
        desc: b"pre desc".to_vec(),
    }
}

fn c5_names() -> Vec<(Vec<u8>, Vec<u8>)> {
    let p = |a: &[u8], b: &[u8]| (a.to_vec(), b.to_vec());
    vec![
        p(b"", b""),
        p(b"Plain name", b"Plain description"),
        p(b"pipe|name", b"pipe|desc"),
        p(b"new\nline", b"new\nline"),
        p(b"caf\xc3\xa9 \xf0\x9f\x98\x80", b"\xe6\x97\xa5\xe6\x9c\xac"),
        (vec![b'n'; 63], vec![b'd'; 127]),
        (vec![b'n'; 64], vec![b'd'; 128]),
        (vec![b'n'; 255], vec![b'd'; 300]),
        p(b"  leading and trailing  ", b"  leading and trailing  "),
        p(b"tab\there", b"tab\there"),
        p(b"cr\r", b"cr\r"),
        p(b"del\x7f", b"del\x7f"),
        // The C++ literal "\x01\x02ctrl" reads `\x02c` as one hex escape
        // (0x2c, ','); "\x1f" "ctrl" is split, so it keeps its "c".
        p(b"\x01,trl", b"\x1fctrl"),
        p(b"|", b"|"),
        p(b"name with spaces", b""),
        p(b"<llsd>", b"</llsd>"),
    ]
}

fn c5_item_cases() -> Vec<(String, ItemF)> {
    let mut v = Vec::new();
    let b = base_item();
    for at in ASSET_TYPES {
        let inv = if (0..60).contains(&at) { InventoryType::default_for_asset_type(AssetType(at)).0 } else { 255 };
        v.push((format!("c5.at.{at}"), ItemF { ty: at, inv_type: inv, ..b.clone() }));
    }
    for it in INV_TYPES {
        v.push((format!("c5.it.{it}"), ItemF { inv_type: it, ..b.clone() }));
    }
    let assets = [NUL, u(0xa3), MAGIC, not_magic()];
    let asset_names = ["null", "a3", "magic", "notmagic"];
    let types = [(6, 6), (7, 7), (3, 3), (10, 10)];
    for m in BASE_MASKS {
        for ai in 0..4 {
            for (t, it) in types {
                let mut f = b.clone();
                f.perm.base = m;
                f.asset = assets[ai];
                f.ty = t;
                f.inv_type = it;
                v.push((format!("c5.mask.{}.{}.{t}", x8(m), asset_names[ai]), f));
            }
        }
    }
    let runtimes: [Vec<u8>; 4] = [Vec::new(), b"luau".to_vec(), vec![b'r'; 60], b"lu au".to_vec()];
    for th in 0..2 {
        for fv in 0..2 {
            for (ri, rt) in runtimes.iter().enumerate() {
                for t in [10, 7] {
                    let f = ItemF {
                        thumb: if th == 1 { u(0xb1) } else { NUL },
                        fav: fv != 0,
                        runtime: rt.clone(),
                        ty: t,
                        inv_type: t,
                        ..b.clone()
                    };
                    v.push((format!("c5.meta.{th}{fv}{ri}.{t}"), f));
                }
            }
        }
    }
    for fl in FLAGS {
        for (t, it) in [(6, 6), (5, 18), (56, 25)] {
            v.push((format!("c5.flags.{}.{t}", x8(fl)), ItemF { flags: fl, ty: t, inv_type: it, ..b.clone() }));
        }
    }
    for st in 0..=3 {
        for p in PRICES {
            v.push((format!("c5.sale.{st}.{p}"), ItemF { sale_type: st, sale_price: p, ..b.clone() }));
        }
    }
    for st in [-1, 4, 200, 255] {
        v.push((format!("c5.sale.{st}.10"), ItemF { sale_type: st, ..b.clone() }));
    }
    for (i, (n, ds)) in c5_names().into_iter().enumerate() {
        v.push((format!("c5.name.{i}"), ItemF { name: n, desc: ds, ..b.clone() }));
    }
    for dt in DATES {
        v.push((format!("c5.date.{dt}"), ItemF { date: dt, ..b.clone() }));
    }
    {
        let mut f = b.clone();
        f.perm.owner = NUL;
        f.perm.group_owned = true;
        v.push(("c5.groupowned".to_owned(), f));
        let mut f = b.clone();
        f.perm = PermF::default();
        v.push(("c5.perm.default".to_owned(), f));
        let mut f = b.clone();
        f.perm.group_mask = PERM_ALL;
        f.perm.everyone = PERM_ALL;
        f.perm.next = PERM_ALL;
        v.push(("c5.perm.unfixed".to_owned(), f));
    }
    {
        let mut f = ItemF { ty: 24, inv_type: -1, asset: u(0xc5), ..b.clone() };
        v.push(("c5.link".to_owned(), f.clone()));
        f.ty = 25;
        f.inv_type = 8;
        v.push(("c5.folderlink".to_owned(), f));
    }
    v
}

// ---------------------------------------------------------------------------
// Categories (the tool's prev_cat, c5_cat_cases)
// ---------------------------------------------------------------------------

fn prev_cat() -> CatF {
    CatF {
        id: u(0x31),
        parent: u(0x32),
        thumb: u(0x34),
        ty: 8,
        pref: 5,
        fav: true,
        date: 99,
        runtime: b"pre-rt".to_vec(),
        name: b"pre folder".to_vec(),
        owner: u(0x33),
        version: 3,
    }
}

fn c5_cat_cases() -> Vec<(String, CatF)> {
    let mut v = Vec::new();
    let b = CatF {
        id: u(0xd1),
        parent: u(0xd2),
        ty: 8,
        name: b"Folder name".to_vec(),
        owner: u(0xd3),
        version: 7,
        ..CatF::default()
    };
    for p in -1..=101 {
        v.push((format!("c5c.pref.{p}"), CatF { pref: p, ..b.clone() }));
    }
    v.push(("c5c.pref.127".to_owned(), CatF { pref: 127, ..b.clone() }));
    v.push(("c5c.pref.-128".to_owned(), CatF { pref: -128, ..b.clone() }));
    for p in [0, 8, 46, -1, 127, 58] {
        for k in 1..4 {
            let f = CatF { pref: p, thumb: if k & 1 != 0 { u(0xb2) } else { NUL }, fav: k & 2 != 0, ..b.clone() };
            v.push((format!("c5c.meta.{p}.{k}"), f));
        }
    }
    for (i, (n, _)) in c5_names().into_iter().enumerate() {
        v.push((format!("c5c.name.{i}"), CatF { pref: 0, name: n, ..b.clone() }));
    }
    for t in [-1, 255, 6, 24] {
        v.push((format!("c5c.type.{t}"), CatF { pref: 0, ty: t, ..b.clone() }));
    }
    v.push(("c5c.runtime".to_owned(), CatF { runtime: b"luau".to_vec(), thumb: u(0xb2), ..b.clone() }));
    v.push(("c5c.nullids".to_owned(), CatF { id: NUL, parent: NUL, owner: NUL, version: -1, ..b.clone() }));
    v
}


// ---------------------------------------------------------------------------
// The LLSD input values (the tool's llsd_values)
// ---------------------------------------------------------------------------

/// `LLSD().with(key, value)`.
fn with(key: &str, v: Value) -> Value {
    let mut m = Value::map();
    insert(&mut m, key, v);
    m
}

fn sv(v: &str) -> Value {
    Value::string(v)
}

/// `u(U(tag))`: the UUID's lower-case dashed string.
fn ustr(tag: u8) -> String {
    uuid(&u(tag))
}

/// `llsd_values()`: the 48 named values of C7 and C8.
fn llsd_values() -> Vec<(&'static str, Value)> {
    let mut perm_map = Value::map();
    insert(&mut perm_map, "base_mask", Value::Integer(0x7fff7fff));
    insert(&mut perm_map, "owner_id", Value::Uuid(u(0x77)));
    let mut sale_map = Value::map();
    insert(&mut sale_map, "sale_type", sv("copy"));
    insert(&mut sale_map, "sale_price", Value::Integer(50));
    let script_map = with("script", with("runtime", sv("luau")));
    let arr = Value::Array(vec![Value::Integer(1), sv("x")]);
    let date = date_parse(b"2026-10-22T10:00:00Z").expect("a valid date");
    vec![
        ("undef", Value::Undefined),
        ("bool.t", Value::Boolean(true)),
        ("bool.f", Value::Boolean(false)),
        ("int.0", Value::Integer(0)),
        ("int.1", Value::Integer(1)),
        ("int.-1", Value::Integer(-1)),
        ("int.7", Value::Integer(7)),
        ("int.255", Value::Integer(255)),
        ("int.256", Value::Integer(256)),
        ("int.300", Value::Integer(300)),
        ("int.max", Value::Integer(i32::MAX)),
        ("int.min", Value::Integer(i32::MIN)),
        ("real.0", Value::Real(0.0)),
        ("real.1.5", Value::Real(1.5)),
        ("real.-1", Value::Real(-1.0)),
        ("real.1e10", Value::Real(1e10)),
        ("real.300.7", Value::Real(300.7)),
        ("str.empty", sv("")),
        ("str.texture", sv("texture")),
        ("str.lsltext", sv("lsltext")),
        ("str.script", sv("script")),
        ("str.notecard", sv("notecard")),
        ("str.invalid", sv("invalid")),
        ("str.-1", sv("-1")),
        ("str.junk", sv("junk")),
        ("str.sp7", sv(" 7")),
        ("str.0x10", sv("0x10")),
        ("str.7", sv("7")),
        ("str.uuid", sv(&ustr(0x75))),
        ("str.pipes", Value::String(b"name|with|pipes\x01\xc3\xa9".to_vec())),
        ("uuid.null", Value::Uuid(NUL)),
        ("uuid.75", Value::Uuid(u(0x75))),
        ("date", Value::Date(date)),
        ("uri", Value::Uri(b"http://example.com/x".to_vec())),
        ("bin0", Value::Binary(Vec::new())),
        ("bin3", Value::Binary(vec![1, 2, 3])),
        ("bin4", Value::Binary(vec![0x00, 0x00, 0x01, 0x01])),
        ("bin5", Value::Binary(vec![0x80, 0, 0, 1, 0xff])),
        ("map.empty", Value::map()),
        ("map.asset_id", with("asset_id", Value::Uuid(u(0x76)))),
        ("map.toggled.t", with("toggled", Value::Boolean(true))),
        ("map.toggled.0", with("toggled", Value::Integer(0))),
        ("map.runtime", with("runtime", sv("luau"))),
        ("map.script", script_map),
        ("map.perm", perm_map),
        ("map.sale", sale_map),
        ("array.empty", Value::array()),
        ("array.1x", arr),
    ]
}


/// `LLSD::erase(key)`.
fn erase(sd: &mut Value, key: &str) {
    if let Value::Map(m) = sd {
        m.remove(key.as_bytes());
    }
}

// ---------------------------------------------------------------------------
// C8 (the tool's c8_category_llsd)
// ---------------------------------------------------------------------------

const C8_KEYS: [&str; 13] = [
    "category_id", "item_id", "cat_id", "parent_id", "thumbnail", "thumbnail_id", "favorite", "type", "type_default",
    "preferred_type", "name", "version", "owner_id",
];

/// The keys whose viewer `importLLSD` is recorded too.
const C8_VKEYS: [&str; 5] = ["owner_id", "version", "favorite", "name", "cat_id"];

fn c8_lines() -> Lines {
    let mut s = Lines::default();
    let values = llsd_values();
    for key in C8_KEYS {
        for (name, v) in &values {
            let sd = with(key, v.clone());
            let cs = format!("c8.{key}.{name}");
            let mut c = make_cat(&prev_cat());
            let ok = c.from_llsd(&sd);
            s.emit(&cs, "fromllsd", &with_ret(ok, cat_cols(&c)));
            let mut m = make_cat(&prev_cat());
            let r = m.import_llsd(key.as_bytes(), v);
            m.import_llsd_map(&sd); // the map reader on top (same key again)
            s.emit(&cs, "importllsd", &with_ret(r, cat_cols(&m)));
            if C8_VKEYS.contains(&key) {
                let mut vc = make_vcat(&prev_cat());
                let vr = vc.import_llsd(key.as_bytes(), v);
                s.emit(&cs, "vimportllsd", &with_ret(vr, vcat_cols(&vc)));
            }
        }
    }
    // thumbnail with thumbnail_id.
    for k in 0..4 {
        let mut sd = with("thumbnail", if k & 1 != 0 { with("asset_id", Value::Uuid(u(0x88))) } else { Value::map() });
        if k & 2 != 0 {
            insert(&mut sd, "thumbnail_id", Value::Uuid(u(0x89)));
        }
        let mut c = make_cat(&prev_cat());
        c.from_llsd(&sd);
        s.emit(&format!("c8.thumbs.{k}"), "fromllsd", &cat_cols(&c));
    }
    // type with type_default.
    {
        let tv: Vec<(&str, Value)> =
            vec![("int", Value::Integer(46)), ("str", sv("46")), ("strname", sv("current")), ("real", Value::Real(46.9))];
        for (an, a) in &tv {
            for (bn, b) in &tv {
                let mut sd = with("type", a.clone());
                insert(&mut sd, "type_default", b.clone());
                let mut c = make_cat(&prev_cat());
                c.from_llsd(&sd);
                s.emit(&format!("c8.typepair.{an}.{bn}"), "fromllsd", &cat_cols(&c));
            }
        }
    }
    // One object reused across records (the legacy fetch loop and the cache
    // loader's dialect).
    {
        let cases = c5_cat_cases();
        let mut c = make_cat(&prev_cat());
        for k in 0..6usize {
            let mut sd = make_cat(&cases[(k * 29) % cases.len()].1).to_ais_create_llsd();
            if k % 2 != 0 {
                erase(&mut sd, "name");
            }
            if k % 3 == 1 {
                erase(&mut sd, "thumbnail");
            }
            c.from_llsd(&sd);
            s.emit(&format!("c8.reuse.{k}"), "fromllsd", &cat_cols(&c));
        }
        let mut vc = make_vcat(&prev_cat());
        for k in 0..6usize {
            let mut sd = Value::Undefined;
            make_vcat(&cases[(k * 31) % cases.len()].1).export_llsd(&mut sd);
            if k % 2 != 0 {
                erase(&mut sd, "version");
            }
            if k % 3 == 2 {
                erase(&mut sd, "preferred_type");
            }
            vc.import_llsd_map(&sd);
            s.emit(&format!("c8.vreuse.{k}"), "vimportmap", &vcat_cols(&vc));
        }
    }
    s
}

#[test]
fn c8_category_llsd() {
    let expected = read_fixture("c8_category_llsd.txt");
    let actual = c8_lines();
    assert_replay("c8_category_llsd.txt", &expected, &actual.0);
}
