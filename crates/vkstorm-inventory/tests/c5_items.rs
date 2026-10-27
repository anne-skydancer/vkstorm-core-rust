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
use vkstorm_inventory::category::{Category, ViewerCategory};
use vkstorm_inventory::item::Item;
use vkstorm_inventory::legacy::reader::Stream;
use vkstorm_inventory::llsd::coerce::{as_integer, as_string, get};
use vkstorm_inventory::object::Object;
use vkstorm_inventory::perms::{Permissions, PERM_ALL};
use vkstorm_inventory::sale::{SaleInfo, SaleType};
use vkstorm_inventory::types::{AssetType, FolderType, InventoryType};
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

// ---------------------------------------------------------------------------
// Categories and objects
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
