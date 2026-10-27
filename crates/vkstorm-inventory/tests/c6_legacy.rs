//! C6 malformed legacy input: replays every line of
//! `fixtures/model/c6_legacy.txt` from the crate, as `c6_legacy()` and
//! `c6_run()` in `llinventory_tool.cpp` write them from the frozen C++.
//!
//! Each case's `input` line names the readers and holds the input; the
//! readers are the item (`I`), object (`O`), category (`C`), permissions
//! (`P`), sale info (`S`) and notecard (`N`) readers, each started from the
//! fixed previous record over a fresh stream. The replay prints its matches
//! by reader.

mod common;

use std::collections::BTreeMap;

use common::*;
use vkstorm_inventory::category::Category;
use vkstorm_inventory::item::Item;
use vkstorm_inventory::legacy::notecard::Notecard;
use vkstorm_inventory::legacy::reader::Stream;
use vkstorm_inventory::object::Object;
use vkstorm_inventory::perms::{Permissions, PERM_ALL, PERM_COPY, PERM_MODIFY, PERM_TRANSFER};
use vkstorm_inventory::sale::{SaleInfo, SaleType};
use vkstorm_inventory::types::{AssetType, FolderType, InventoryType};

const FIXTURE: &str = "c6_legacy.txt";

/// The inverse of `esc`: `\\` and `\xHH`.
fn unescape(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && b.get(i + 1) == Some(&b'\\') {
            out.push(b'\\');
            i += 2;
        } else if b[i] == b'\\' && b.get(i + 1) == Some(&b'x') {
            out.push(u8::from_str_radix(&s[i + 2..i + 4], 16).expect("\\xHH"));
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The previous records (README "Fixed inputs") and the tool's columns
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn make_perm(creator: [u8; 16], owner: [u8; 16], last: [u8; 16], group: [u8; 16], masks: [u32; 5], group_owned: bool) -> Permissions {
    let mut p = Permissions::default();
    p.init(&creator, &owner, &last, &group);
    [p.base, p.owner_mask, p.group_mask, p.everyone, p.next] = masks;
    p.yes_really_set_owner(&owner, group_owned);
    p
}

/// `prev_item()`, built raw.
fn prev_item() -> Item {
    Item {
        object: Object {
            id: u(0x11),
            parent: u(0x12),
            thumbnail: u(0x14),
            asset_type: AssetType(7),
            favorite: true,
            creation_date: 1234567890,
            runtime: b"pre-runtime".to_vec(),
            name: b"pre name".to_vec(),
        },
        permissions: make_perm(u(0x21), u(0x22), u(0x23), u(0x24), [PERM_ALL, 0x0008e000, 0x00002000, 0, 0x00082000], false),
        asset: u(0x13),
        description: b"pre desc".to_vec(),
        sale_info: SaleInfo { sale_type: SaleType(2), price: 25 },
        inv_type: InventoryType(7),
        flags: 0x101,
    }
}

/// `prev_cat()` as an object.
fn prev_obj() -> Object {
    Object {
        id: u(0x31),
        parent: u(0x32),
        thumbnail: u(0x34),
        asset_type: AssetType(8),
        favorite: true,
        creation_date: 99,
        runtime: b"pre-rt".to_vec(),
        name: b"pre folder".to_vec(),
    }
}

/// `prev_cat()` as a category.
fn prev_cat() -> Category {
    Category { object: prev_obj(), preferred_type: FolderType(5) }
}

fn item_cols(it: &Item) -> Vec<String> {
    let o = &it.object;
    let mut v = vec![uuid(&o.id), uuid(&o.parent), uuid(&it.asset), uuid(&o.thumbnail), d(o.asset_type.0), d(it.inv_type.0), x8(it.flags)];
    v.extend(perm_cols(&it.permissions));
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

fn obj_cols(o: &Object) -> Vec<String> {
    vec![uuid(&o.id), uuid(&o.parent), d(o.asset_type.0), uuid(&o.thumbnail), bl(o.favorite), d(o.creation_date), esc(&o.runtime), esc(&o.name)]
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

fn ret_stream(ret: bool, s: &Stream<'_>, src: &[u8]) -> Vec<String> {
    let mut v = stream_cols(s, src);
    v.insert(0, bl(ret));
    v
}

/// `perm_invariants`: the `fix()` invariants, one character each.
fn perm_invariants(p: &Permissions) -> String {
    let (b, o, g, e, n) = (p.base, p.owner_mask, p.group_mask, p.everyone, p.next);
    let c = |bad: bool| if bad { '0' } else { '1' };
    [
        c(o & !b != 0),
        c(g & !o != 0),
        c(e & !o != 0),
        c(n & !b != 0),
        c(e & PERM_MODIFY != 0),
        c(b & PERM_TRANSFER == 0 && !p.group_owned && (g | e) & PERM_COPY != 0),
    ]
    .iter()
    .collect()
}

/// `no_widening`: no mask bit and no group ownership beyond the baseline's.
fn no_widening(p: &Permissions, base: &Permissions) -> String {
    let c = |bad: bool| if bad { '0' } else { '1' };
    [
        c(p.base & !base.base != 0),
        c(p.owner_mask & !base.owner_mask != 0),
        c(p.group_mask & !base.group_mask != 0),
        c(p.everyone & !base.everyone != 0),
        c(p.next & !base.next != 0),
        c(p.group_owned && !base.group_owned),
    ]
    .iter()
    .collect()
}

/// `long_text`: escaped up to 4096 bytes, else its length and SHA-256.
fn long_text(t: &[u8]) -> String {
    if t.len() <= 4096 {
        esc(t)
    } else {
        format!("len={};sha256={}", t.len(), sha256_hex(t))
    }
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// The corrupt-input cases and their baselines: the standard item (the
/// `c6.good` input) or the bare item around an empty permissions block.
fn baseline(case: &str, std_item: &[u8]) -> Option<Vec<u8>> {
    match case {
        "c6.perm.group_owned.junk" | "c6.perm.mask.stale" => Some(std_item.to_vec()),
        "c6.perm.mask.first_fails" | "c6.perm.group_owned.first_fails" | "c6.perm.id.first_empty" => {
            Some(b"\t{\n\tpermissions 0\n\t{\n\t}\n\t}\nNEXT\tline\n".to_vec())
        }
        _ => None,
    }
}

/// The notecard limit of a case: `c6.nc.limit64.*` is constructed with 64.
fn max_text(case: &str) -> i32 {
    if case.starts_with("c6.nc.limit64.") { 64 } else { Notecard::MAX_SIZE }
}

fn run_case(cs: &str, readers: &str, input: &[u8], base: Option<&[u8]>, s: &mut Lines) {
    for r in readers.chars() {
        let mut is = Stream::new(input);
        match r {
            'I' => {
                let mut it = prev_item();
                let ok = it.import_legacy_stream(&mut is);
                s.emit(cs, "item", &item_cols(&it));
                if let Some(b) = base {
                    let mut bit = prev_item();
                    bit.import_legacy_stream(&mut Stream::new(b));
                    s.emit(cs, "item.invariants", &[perm_invariants(&it.permissions)]);
                    s.emit(cs, "item.no_widening", &[no_widening(&it.permissions, &bit.permissions)]);
                }
                s.emit(cs, "item.stream", &ret_stream(ok, &is, input));
            }
            'O' => {
                let mut o = prev_obj();
                let ok = o.import_legacy_stream(&mut is);
                s.emit(cs, "obj", &obj_cols(&o));
                s.emit(cs, "obj.stream", &ret_stream(ok, &is, input));
            }
            'C' => {
                let mut c = prev_cat();
                let ok = c.import_legacy_stream(&mut is);
                s.emit(cs, "cat", &cat_cols(&c));
                s.emit(cs, "cat.stream", &ret_stream(ok, &is, input));
            }
            'P' => {
                let mut p = prev_item().permissions;
                let ok = p.import_legacy_stream(&mut is);
                s.emit(cs, "perm", &ret_perm_cols(ok, &p));
                if let Some(b) = base {
                    let mut bp = prev_item().permissions;
                    bp.import_legacy_stream(&mut Stream::new(b));
                    s.emit(cs, "perm.invariants", &[perm_invariants(&p)]);
                    s.emit(cs, "perm.no_widening", &[no_widening(&p, &bp)]);
                }
                s.emit(cs, "perm.stream", &stream_cols(&is, input));
            }
            'S' => {
                let mut sale = SaleInfo { sale_type: SaleType(1), price: 77 };
                let (mut has, mut pm) = (true, 0xa5a5_a5a5u32);
                let ok = sale.import_legacy_stream(&mut is, &mut has, &mut pm);
                let mut cols = sale_cols(&sale);
                cols.extend([bl(has), x8(pm), bl(ok)]);
                s.emit(cs, "sale", &cols);
                s.emit(cs, "sale.stream", &stream_cols(&is, input));
            }
            'N' => {
                let mut nc = Notecard::new(max_text(cs));
                let ok = nc.import_stream(&mut is);
                s.emit(
                    cs,
                    "notecard",
                    &[bl(ok), d(nc.version), d(nc.embedded_version), d(nc.items.len() as i64), long_text(&nc.text)],
                );
                for (k, it) in nc.items.iter().enumerate() {
                    s.emit(cs, &format!("notecard.item.{k}"), &item_cols(it));
                }
                s.emit(cs, "notecard.stream", &stream_cols(&is, input));
                if ok {
                    let mut out = Vec::new();
                    let eok = nc.export_stream(&mut out);
                    s.emit(cs, "notecard.export", &[bl(eok), long_text(&out)]);
                }
            }
            other => panic!("{cs}: unknown reader {other}"),
        }
    }
}

/// `c6.nc.export`: a notecard of C5 items 0, 40 and 120 (`c5.at.-1`,
/// `c5.it.6`, `c5.mask.7fffdfff.magic.7`), built raw from `base_item()`.
fn c6_nc_export(s: &mut Lines) {
    let base = |ty: i32, inv: i32, base_mask: u32, asset: [u8; 16]| Item {
        object: Object {
            id: u(0xa1),
            parent: u(0xa2),
            asset_type: AssetType(ty),
            creation_date: 1700000000,
            name: b"Base item".to_vec(),
            ..Object::default()
        },
        permissions: make_perm(id_c(), id_a(), id_b(), id_g(), [base_mask, PERM_ALL, 0, 0, 0x00082000], false),
        asset,
        description: b"Base description".to_vec(),
        sale_info: SaleInfo { sale_type: SaleType(0), price: 10 },
        inv_type: InventoryType(inv),
        flags: 0,
    };
    let mut nc = Notecard::default();
    nc.items = vec![base(-1, 255, PERM_ALL, u(0xa3)), base(6, 6, PERM_ALL, u(0xa3)), base(7, 7, 0x7fffdfff, MAGIC)];
    nc.text = b"Text with an embedded item \xf4\x80\x80\x80 marker\n".to_vec();
    let mut out = Vec::new();
    let ok = nc.export_stream(&mut out);
    s.emit("c6.nc.export", "notecard.export", &[bl(ok), esc(&out)]);
    let mut back = Notecard::default();
    let rok = back.import_stream(&mut Stream::new(&out));
    s.emit("c6.nc.export", "notecard.reimport", &[bl(rok), d(back.version), d(back.items.len() as i64), esc(&back.text)]);
    for (k, it) in back.items.iter().enumerate() {
        s.emit("c6.nc.export", &format!("notecard.reimport.item.{k}"), &item_cols(it));
    }
}

fn reader_of(field: &str) -> &'static str {
    match field.split('.').next().unwrap_or("") {
        "input" => "input",
        "item" => "I item",
        "obj" => "O object",
        "cat" => "C category",
        "perm" => "P permissions",
        "sale" => "S sale",
        "notecard" => "N notecard",
        _ => "other",
    }
}

#[test]
fn c6_legacy() {
    let expected = read_fixture(FIXTURE);
    let std_item = expected
        .iter()
        .map(|l| split_tabs(l))
        .find(|v| v[0] == "c6.good" && v[1] == "input")
        .map(|v| unescape(v[3]))
        .expect("c6.good input");
    let mut s = Lines::default();
    for line in &expected {
        let v = split_tabs(line);
        if v.len() == 4 && v[1] == "input" {
            let input = unescape(v[3]);
            s.emit(v[0], "input", &[v[2].to_owned(), esc(&input)]);
            run_case(v[0], v[2], &input, baseline(v[0], &std_item).as_deref(), &mut s);
        }
    }
    c6_nc_export(&mut s);

    let mut tally: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    if expected.len() == s.0.len() {
        for (e, a) in expected.iter().zip(&s.0) {
            let t = tally.entry(reader_of(split_tabs(e)[1])).or_default();
            t.1 += 1;
            if e == a {
                t.0 += 1;
            }
        }
        for (k, (m, n)) in &tally {
            println!("c6 {k}: {m}/{n} lines match");
        }
    }
    assert_replay(FIXTURE, &expected, &s.0);
}

#[test]
fn notecard_round_trip_keeps_text_bytes() {
    let mut nc = Notecard::new(64);
    nc.text = b"a\0b\n".to_vec();
    let mut out = Vec::new();
    assert!(nc.export_stream(&mut out));
    let mut back = Notecard::new(64);
    assert!(back.import_stream(&mut Stream::new(&out)));
    // std::string(char*) stops at the NUL.
    assert_eq!(back.text, b"a");
}
