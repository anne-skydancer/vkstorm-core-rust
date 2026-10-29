//! C9 message blocks: regenerates every line of
//! `fixtures/model/c9_messages.txt` from the crate, as `c9_messages()` in
//! `llinventory_tool.cpp` writes them from the frozen C++ and the real
//! message system:
//!
//! - for every C5 item case, the `UpdateInventoryItem` and
//!   `UpdateTaskInventory` packets (the viewer `packMessage`, TransactionID),
//!   the `BulkUpdateInventory` packet (the base `packMessage`, AssetID), and
//!   that packet unpacked into a new item and into the previous item with
//!   the CRC verdict;
//! - the `c9.udp.*` raw `ItemData` blocks and `c9.udp.multi`;
//! - `UpdateInventoryFolder` from the viewer category `packMessage` for every
//!   C5 category, unpacked into the previous category (`c5c.*`), and the raw
//!   folder block `c9.folder.raw`;
//! - through `ObjectProperties`: `LLAggregatePermissions` of all 256 bytes,
//!   repacked (`c9.agg.*`) and aggregated pairwise (`c9.aggagg.*`),
//!   `LLCategory` (`c9.category.*`) and `LLSaleInfo` (`c9.sale.*`).
//!
//! Messages are built and read with `vkstorm-messages` over the viewer's
//! `message_template.msg`, which that crate's tests carry as a fixture. A
//! packet is the built message from its message number on, before
//! zero-coding, without the 6-byte header.

mod common;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use common::*;
use vkstorm_inventory::aggregate::AggregatePermissions;
use vkstorm_inventory::category::Category;
use vkstorm_inventory::category_obj::Category as LLCategory;
use vkstorm_inventory::item::Item;
use vkstorm_inventory::message::{
    FolderBlockKeys, ItemBlock, ItemBlockKeys, SaleKeys, pack_aggregate, pack_category, pack_item,
    pack_item_block, pack_llcategory, pack_sale, pack_viewer_category, pack_viewer_item,
    unpack_aggregate, unpack_category, unpack_item, unpack_llcategory, unpack_sale,
};
use vkstorm_inventory::perms::PERM_ALL;
use vkstorm_inventory::perms::{PERM_COPY, PERM_MODIFY, PERM_TRANSFER};
use vkstorm_inventory::sale::{SaleInfo, SaleType};
use vkstorm_messages::{
    Builder, MAX_BUFFER_SIZE, PACKET_ID_SIZE, Reader, Template, Tokenizer, VarType, parse_template,
};

// ---------------------------------------------------------------------------
// The message system (the tool's built_bytes, packet_hex, dispatch)
// ---------------------------------------------------------------------------

/// The viewer's message template, as the `vkstorm-messages` tests carry it
/// (`scripts/messages/message_template.msg`), with index keys.
struct Messages {
    by_name: HashMap<String, Arc<Template>>,
    builder: Builder,
    reader: Reader,
}

const AGENT: u8 = 0xf1;
const SESSION: u8 = 0xf2;
const TRANSACTION: u8 = 0xf3;
/// `TASK_INVENTORY_ITEM_KEY`.
const TASK_INVENTORY_ITEM_KEY: u8 = 0;

impl Messages {
    fn load() -> Messages {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../vkstorm-messages/tests/fixtures/message_template.msg"
        );
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let file = parse_template(&mut Tokenizer::new(&text)).expect("message template parses");
        let by_name = file
            .messages
            .into_iter()
            .map(|t| (t.name.clone(), Arc::new(t.with_index_keys())))
            .collect();
        Messages {
            by_name,
            builder: Builder::new(),
            reader: Reader::new(),
        }
    }

    fn template(&self, name: &str) -> Arc<Template> {
        self.by_name
            .get(name)
            .unwrap_or_else(|| panic!("no message {name}"))
            .clone()
    }

    /// `newMessageFast`: the message's template.
    fn new_message(&mut self, name: &str) -> Arc<Template> {
        let t = self.template(name);
        self.builder.new_message(t.clone());
        t
    }

    /// `nextBlockFast` by name.
    fn next_block(&mut self, t: &Template, block: &str) {
        self.builder.next_block(block_key(t, block)).unwrap();
    }

    /// `add*Fast` of a fixed-size variable by name.
    fn add(&mut self, t: &Template, block: &str, var: &str, data: &[u8], ty: VarType) {
        let key = t
            .block_by_name(block)
            .unwrap()
            .variable_by_name(var)
            .unwrap()
            .key;
        self.builder.add_data(key, data, ty).unwrap();
    }

    /// `built_bytes`: the built message, its header zeroed.
    fn built_bytes(&mut self) -> Vec<u8> {
        let mut buf = vec![0u8; MAX_BUFFER_SIZE];
        let n = self
            .builder
            .build_message(&mut buf, 0)
            .expect("message builds");
        buf.truncate(n);
        self.builder.clear_message();
        for b in buf.iter_mut().take(PACKET_ID_SIZE) {
            *b = 0;
        }
        buf
    }

    /// `dispatch`: the packet into the reader.
    fn dispatch(&mut self, name: &str, bytes: &[u8]) -> Arc<Template> {
        let t = self.template(name);
        self.reader.set_message(t.clone(), bytes.len() as i32);
        let outcome = self.reader.decode_data(bytes);
        assert!(
            outcome.ran_off_end.is_empty(),
            "{name}: {:?}",
            outcome.ran_off_end
        );
        t
    }
}

fn block_key(t: &Template, block: &str) -> u64 {
    t.block_by_name(block)
        .unwrap_or_else(|| panic!("no block {block} in {}", t.name))
        .key
}

fn item_keys(t: &Template, block: &str) -> ItemBlockKeys {
    ItemBlockKeys::from_block(t.block_by_name(block).unwrap()).expect("an item block")
}

/// `packet_hex`.
fn packet_hex(b: &[u8]) -> String {
    if b.len() > PACKET_ID_SIZE {
        hex(&b[PACKET_ID_SIZE..])
    } else {
        String::new()
    }
}

fn pack_update_inventory_item(m: &mut Messages, item: &Item) -> Vec<u8> {
    let t = m.new_message("UpdateInventoryItem");
    m.next_block(&t, "AgentData");
    m.add(&t, "AgentData", "AgentID", &u(AGENT), VarType::LLUUID);
    m.add(&t, "AgentData", "SessionID", &u(SESSION), VarType::LLUUID);
    m.add(
        &t,
        "AgentData",
        "TransactionID",
        &u(TRANSACTION),
        VarType::LLUUID,
    );
    m.next_block(&t, "InventoryData");
    m.add(
        &t,
        "InventoryData",
        "CallbackID",
        &0u32.to_le_bytes(),
        VarType::U32,
    );
    pack_viewer_item(
        &mut m.builder,
        &item_keys(&t, "InventoryData"),
        item,
        &u(TRANSACTION),
    )
    .unwrap();
    m.built_bytes()
}

fn pack_update_task_inventory(m: &mut Messages, item: &Item) -> Vec<u8> {
    let t = m.new_message("UpdateTaskInventory");
    m.next_block(&t, "AgentData");
    m.add(&t, "AgentData", "AgentID", &u(AGENT), VarType::LLUUID);
    m.add(&t, "AgentData", "SessionID", &u(SESSION), VarType::LLUUID);
    m.next_block(&t, "UpdateData");
    m.add(
        &t,
        "UpdateData",
        "LocalID",
        &0x0102_0304u32.to_le_bytes(),
        VarType::U32,
    );
    m.add(
        &t,
        "UpdateData",
        "Key",
        &[TASK_INVENTORY_ITEM_KEY],
        VarType::U8,
    );
    m.next_block(&t, "InventoryData");
    pack_viewer_item(
        &mut m.builder,
        &item_keys(&t, "InventoryData"),
        item,
        &u(TRANSACTION),
    )
    .unwrap();
    m.built_bytes()
}

fn bulk_header(m: &mut Messages) -> Arc<Template> {
    let t = m.new_message("BulkUpdateInventory");
    m.next_block(&t, "AgentData");
    m.add(&t, "AgentData", "AgentID", &u(AGENT), VarType::LLUUID);
    m.add(
        &t,
        "AgentData",
        "TransactionID",
        &u(TRANSACTION),
        VarType::LLUUID,
    );
    t
}

fn pack_bulk_update(m: &mut Messages, item: &Item) -> Vec<u8> {
    let t = bulk_header(m);
    m.next_block(&t, "ItemData");
    m.add(
        &t,
        "ItemData",
        "CallbackID",
        &7u32.to_le_bytes(),
        VarType::U32,
    );
    pack_item(&mut m.builder, &item_keys(&t, "ItemData"), item).unwrap();
    m.built_bytes()
}

/// `pack_bulk_raw`: the folder blocks, then the item blocks.
fn pack_bulk_raw(m: &mut Messages, items: &[ItemBlock], folders: &[Category]) -> Vec<u8> {
    let t = bulk_header(m);
    for c in folders {
        m.next_block(&t, "FolderData");
        pack_category(&mut m.builder, &folder_keys(&t), c).unwrap();
    }
    let keys = item_keys(&t, "ItemData");
    for b in items {
        m.next_block(&t, "ItemData");
        m.add(
            &t,
            "ItemData",
            "CallbackID",
            &0u32.to_le_bytes(),
            VarType::U32,
        );
        pack_item_block(&mut m.builder, &keys, b).unwrap();
    }
    m.built_bytes()
}

/// `unpackMessage` of `ItemData` repeat `n` into `item`, as `ret` + record.
fn unpack_cols(m: &Messages, t: &Template, n: i32, item: &mut Item) -> Vec<String> {
    let r = unpack_item(
        &m.reader,
        block_key(t, "ItemData"),
        n,
        &item_keys(t, "ItemData"),
        item,
    )
    .unwrap();
    with_ret(r, item_cols(item))
}

/// The bulk packet unpacked into a new item and into the previous item.
fn emit_unpacks(m: &mut Messages, s: &mut Lines, cs: &str, bytes: &[u8]) {
    let t = m.dispatch("BulkUpdateInventory", bytes);
    let mut fresh = Item::default();
    let mut reused = make_item(&prev_item());
    let a = unpack_cols(m, &t, 0, &mut fresh);
    let b = unpack_cols(m, &t, 0, &mut reused);
    s.emit(cs, "unpack.fresh", &a);
    s.emit(cs, "unpack.reused", &b);
}

/// `udp_from`: the raw block of a record, with the CRC the frozen copy
/// computes for it with a null thumbnail.
fn udp_from(f: &ItemF) -> ItemBlock {
    let mut g = f.clone();
    g.thumb = NUL;
    ItemBlock::from_item(&make_item(&g))
}

fn c9_udp_variants() -> Vec<(String, ItemBlock)> {
    let mut v = Vec::new();
    let b = base_item();
    let base = udp_from(&b);
    v.push(("base".to_owned(), base.clone()));
    let mut x = base.clone();
    x.crc = x.crc.wrapping_add(1);
    v.push(("badcrc".to_owned(), x));
    for t in [-2i8, 127, -128, 26, 24] {
        v.push((
            format!("type.{t}"),
            udp_from(&ItemF {
                ty: i32::from(t),
                ..b.clone()
            }),
        ));
    }
    for t in [100i8, -100, 29, 2, 3, -1] {
        v.push((
            format!("invtype.{t}"),
            udp_from(&ItemF {
                inv_type: i32::from(t),
                ..b.clone()
            }),
        ));
    }
    for st in [4u8, 200, 255, 3] {
        v.push((
            format!("saletype.{st}"),
            udp_from(&ItemF {
                sale_type: i32::from(st),
                ..b.clone()
            }),
        ));
    }
    for p in [-5, i32::MIN, i32::MAX] {
        v.push((
            format!("price.{p}"),
            udp_from(&ItemF {
                sale_price: p,
                ..b.clone()
            }),
        ));
    }
    for (i, (n, ds)) in c5_names().into_iter().enumerate() {
        v.push((
            format!("name.{i}"),
            udp_from(&ItemF {
                name: n,
                desc: ds,
                ..b.clone()
            }),
        ));
    }
    for len in [254usize, 255, 256] {
        v.push((
            format!("strlen.{len}"),
            udp_from(&ItemF {
                name: vec![b'n'; len],
                desc: vec![b'd'; len],
                ..b.clone()
            }),
        ));
    }
    let mut f = b.clone();
    f.perm.group_owned = true;
    f.perm.group_mask = PERM_ALL;
    f.perm.everyone = PERM_ALL;
    v.push(("rawperm".to_owned(), udp_from(&f)));
    v.push((
        "date.-1".to_owned(),
        udp_from(&ItemF {
            date: -1,
            ..b.clone()
        }),
    ));
    v.push((
        "flags.ffffffff".to_owned(),
        udp_from(&ItemF {
            flags: 0xffff_ffff,
            ..b.clone()
        }),
    ));
    v
}

/// The lines of `c9_messages()`, in the recorder's order.
fn c9_item_lines() -> Lines {
    let mut m = Messages::load();
    let mut s = Lines::default();
    for (cs, f) in c5_item_cases() {
        // The viewer item: the record plus the transaction id, which only
        // the viewer packMessage reads.
        let it = make_item(&f);
        let p = pack_update_inventory_item(&mut m, &it);
        s.emit(&cs, "UpdateInventoryItem", &[packet_hex(&p)]);
        let p = pack_update_task_inventory(&mut m, &it);
        s.emit(&cs, "UpdateTaskInventory", &[packet_hex(&p)]);
        let bulk = pack_bulk_update(&mut m, &it);
        s.emit(&cs, "BulkUpdateInventory", &[packet_hex(&bulk)]);
        emit_unpacks(&mut m, &mut s, &cs, &bulk);
    }
    let variants = c9_udp_variants();
    for (name, b) in &variants {
        let cs = format!("c9.udp.{name}");
        let bytes = pack_bulk_raw(&mut m, std::slice::from_ref(b), &[]);
        s.emit(&cs, "packet", &[packet_hex(&bytes)]);
        emit_unpacks(&mut m, &mut s, &cs, &bytes);
    }
    {
        let four: Vec<ItemBlock> = (0..4)
            .map(|k| variants[(k * 5) % variants.len()].1.clone())
            .collect();
        let bytes = pack_bulk_raw(&mut m, &four, &[]);
        s.emit("c9.udp.multi", "packet", &[packet_hex(&bytes)]);
        let t = m.dispatch("BulkUpdateInventory", &bytes);
        let n = m
            .reader
            .number_of_blocks(block_key(&t, "ItemData"))
            .unwrap();
        s.emit("c9.udp.multi", "blocks", &[d(n)]);
        let mut reused = make_item(&prev_item());
        for k in 0..n {
            let cols = unpack_cols(&m, &t, k, &mut reused);
            s.emit("c9.udp.multi", &format!("unpack.{k}"), &cols);
        }
    }
    c9_folder_lines(&mut m, &mut s);
    c9_object_properties_lines(&mut m, &mut s);
    s
}

/// The kind of a line, for the pass counts.
fn kind(case: &str, field: &str) -> String {
    const GROUPS: [&str; 5] = ["c9.folder", "c9.aggagg", "c9.agg", "c9.category", "c9.sale"];
    if let Some(k) = GROUPS.iter().find(|k| case.starts_with(&format!("{k}."))) {
        format!("{k} {field}")
    } else if case.starts_with("c5c.") {
        format!("c5c {field}")
    } else if case.starts_with("c9.udp.") {
        let f = if field.starts_with("unpack.")
            && field != "unpack.fresh"
            && field != "unpack.reused"
        {
            "unpack.n"
        } else {
            field
        };
        format!("c9.udp {f}")
    } else {
        field.to_owned()
    }
}

#[test]
fn c9_messages() {
    let expected = read_fixture("c9_messages.txt");
    let actual = c9_item_lines().0;
    let mut by: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for (i, e) in expected.iter().enumerate() {
        let mut parts = e.split('\t');
        let (case, field) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        let t = by.entry(kind(case, field)).or_insert((0, 0));
        t.1 += 1;
        if actual.get(i) == Some(e) {
            t.0 += 1;
        }
    }
    let parts: Vec<String> = by
        .iter()
        .map(|(k, (m, t))| format!("{k} {m}/{t}"))
        .collect();
    println!("c9_messages.txt: by kind: {}", parts.join(", "));
    assert_replay("c9_messages.txt", &expected, &actual);
}

// ---------------------------------------------------------------------------
// Folders and ObjectProperties
// ---------------------------------------------------------------------------

fn folder_keys(t: &Template) -> FolderBlockKeys {
    FolderBlockKeys::from_block(t.block_by_name("FolderData").unwrap()).unwrap()
}

/// `UpdateInventoryFolder` of every C5 category and `c9.folder.raw`.
fn c9_folder_lines(m: &mut Messages, s: &mut Lines) {
    for (cs, f) in c5_cat_cases() {
        let cat = make_vcat(&f);
        let t = m.new_message("UpdateInventoryFolder");
        m.next_block(&t, "AgentData");
        m.add(&t, "AgentData", "AgentID", &u(AGENT), VarType::LLUUID);
        m.add(&t, "AgentData", "SessionID", &u(SESSION), VarType::LLUUID);
        m.next_block(&t, "FolderData");
        pack_viewer_category(&mut m.builder, &folder_keys(&t), &cat).unwrap();
        let bytes = m.built_bytes();
        s.emit(&cs, "UpdateInventoryFolder", &[packet_hex(&bytes)]);
        let t = m.dispatch("UpdateInventoryFolder", &bytes);
        let mut back = make_cat(&prev_cat());
        unpack_category(
            &m.reader,
            block_key(&t, "FolderData"),
            0,
            &folder_keys(&t),
            &mut back,
        )
        .unwrap();
        s.emit(&cs, "unpack", &cat_cols(&back));
    }
    let raw = CatF {
        id: u(0xd5),
        parent: u(0xd6),
        pref: -100,
        name: b"raw|name\xc3\xa9\x01".to_vec(),
        ..CatF::default()
    };
    let bytes = pack_bulk_raw(m, &[], &[make_cat(&raw)]);
    s.emit("c9.folder.raw", "packet", &[packet_hex(&bytes)]);
    let t = m.dispatch("BulkUpdateInventory", &bytes);
    let mut back = make_cat(&prev_cat());
    unpack_category(
        &m.reader,
        block_key(&t, "FolderData"),
        0,
        &folder_keys(&t),
        &mut back,
    )
    .unwrap();
    s.emit("c9.folder.raw", "unpack", &cat_cols(&back));
}

/// `addStringFast(var, const char*)`: the bytes and a NUL, also for "".
fn add_cstr(m: &mut Messages, t: &Template, var: &str, s: &[u8]) {
    let mut data = s.to_vec();
    data.push(0);
    m.add(t, "ObjectData", var, &data, VarType::Variable);
}

/// The middle of an `ObjectProperties` block (sale type, sale price,
/// aggregate permissions, category): raw values, or added by a closure.
enum Middle<'a> {
    Raw {
        agg: u8,
        category: u32,
        sale_type: u8,
        price: i32,
    },
    Custom(&'a dyn Fn(&mut Messages, &Template)),
}

/// The tool's `object_properties`.
fn object_properties(m: &mut Messages, middle: Middle<'_>) -> Vec<u8> {
    let t = m.new_message("ObjectProperties");
    m.next_block(&t, "ObjectData");
    let od = |m: &mut Messages, var: &str, data: &[u8], ty: VarType| {
        m.add(&t, "ObjectData", var, data, ty)
    };
    od(m, "ObjectID", &u(0x91), VarType::LLUUID);
    od(m, "CreatorID", &u(0x92), VarType::LLUUID);
    od(m, "OwnerID", &u(0x93), VarType::LLUUID);
    od(m, "GroupID", &u(0x94), VarType::LLUUID);
    od(m, "CreationDate", &0u64.to_le_bytes(), VarType::U64);
    od(m, "BaseMask", &PERM_ALL.to_le_bytes(), VarType::U32);
    od(m, "OwnerMask", &PERM_ALL.to_le_bytes(), VarType::U32);
    od(m, "GroupMask", &0u32.to_le_bytes(), VarType::U32);
    od(m, "EveryoneMask", &0u32.to_le_bytes(), VarType::U32);
    od(m, "NextOwnerMask", &PERM_ALL.to_le_bytes(), VarType::U32);
    od(m, "OwnershipCost", &0i32.to_le_bytes(), VarType::S32);
    match middle {
        Middle::Raw {
            agg,
            category,
            sale_type,
            price,
        } => {
            od(m, "SaleType", &[sale_type], VarType::U8);
            od(m, "SalePrice", &price.to_le_bytes(), VarType::S32);
            od(m, "AggregatePerms", &[agg], VarType::U8);
            od(m, "Category", &category.to_le_bytes(), VarType::U32);
        }
        Middle::Custom(f) => f(m, &t),
    }
    od(m, "AggregatePermTextures", &[0], VarType::U8);
    od(m, "AggregatePermTexturesOwner", &[0], VarType::U8);
    od(m, "InventorySerial", &0i16.to_le_bytes(), VarType::S16);
    od(m, "ItemID", &NUL, VarType::LLUUID);
    od(m, "FolderID", &NUL, VarType::LLUUID);
    od(m, "FromTaskID", &NUL, VarType::LLUUID);
    od(m, "LastOwnerID", &NUL, VarType::LLUUID);
    add_cstr(m, &t, "Name", b"obj");
    add_cstr(m, &t, "Description", b"");
    add_cstr(m, &t, "TouchName", b"");
    add_cstr(m, &t, "SitName", b"");
    add_cstr(m, &t, "TextureID", b"");
    m.built_bytes()
}

fn var_key(t: &Template, var: &str) -> u64 {
    t.block_by_name("ObjectData")
        .unwrap()
        .variable_by_name(var)
        .unwrap()
        .key
}

/// `c9.agg.*`, `c9.aggagg.*`, `c9.category.*` and `c9.sale.*`.
fn c9_object_properties_lines(m: &mut Messages, s: &mut Lines) {
    let op = m.template("ObjectProperties");
    let od = block_key(&op, "ObjectData");
    let sale_keys = SaleKeys::from_block(op.block_by_name("ObjectData").unwrap()).unwrap();
    // Aggregate permissions: unpacked, then packed again and read back.
    let mut states = Vec::with_capacity(256);
    for a in 0..=255u8 {
        let bytes = object_properties(
            m,
            Middle::Raw {
                agg: a,
                category: 0,
                sale_type: 0,
                price: 0,
            },
        );
        m.dispatch("ObjectProperties", &bytes);
        let ag = unpack_aggregate(&m.reader, od, 0, var_key(&op, "AggregatePerms")).unwrap();
        states.push(ag);
        let repack = |m: &mut Messages, t: &Template| {
            m.add(t, "ObjectData", "SaleType", &[0], VarType::U8);
            m.add(
                t,
                "ObjectData",
                "SalePrice",
                &0i32.to_le_bytes(),
                VarType::S32,
            );
            pack_aggregate(&mut m.builder, var_key(t, "AggregatePerms"), &ag).unwrap();
            m.add(
                t,
                "ObjectData",
                "Category",
                &0u32.to_le_bytes(),
                VarType::U32,
            );
        };
        let repacked = object_properties(m, Middle::Custom(&repack));
        m.dispatch("ObjectProperties", &repacked);
        let back = m
            .reader
            .get(od, 0, var_key(&op, "AggregatePerms"), 1)
            .unwrap()[0];
        s.emit(
            &format!("c9.agg.{}", x2(u32::from(a))),
            "agg",
            &[
                x2(u32::from(ag.to_u8())),
                d(ag.value(PERM_COPY)),
                d(ag.value(PERM_MODIFY)),
                d(ag.value(PERM_TRANSFER)),
                bl(ag.is_empty()),
                x2(u32::from(back)),
            ],
        );
    }
    for (a, sa) in states.iter().take(64).enumerate() {
        let row: String = states[..64]
            .iter()
            .map(|sb| {
                let mut x: AggregatePermissions = *sa;
                x.aggregate(sb);
                x2(u32::from(x.to_u8()))
            })
            .collect();
        s.emit(&format!("c9.aggagg.{}", x2(a as u32)), "b0..63", &[row]);
    }
    // LLCategory: unpackMessage and unpackMultiMessage (repeat 0).
    for value in [
        0x00000000u32,
        0x01000000,
        0x06000000,
        0x01010000,
        0x06ff01ff,
        0xffffffff,
        0x12345678,
        0x00000001,
    ] {
        let cat = LLCategory::new(value);
        let custom = |m: &mut Messages, t: &Template| {
            m.add(t, "ObjectData", "SaleType", &[0], VarType::U8);
            m.add(
                t,
                "ObjectData",
                "SalePrice",
                &0i32.to_le_bytes(),
                VarType::S32,
            );
            m.add(t, "ObjectData", "AggregatePerms", &[0], VarType::U8);
            pack_llcategory(&mut m.builder, var_key(t, "Category"), cat).unwrap();
        };
        let bytes = object_properties(m, Middle::Custom(&custom));
        m.dispatch("ObjectProperties", &bytes);
        let key = var_key(&op, "Category");
        let c1 = unpack_llcategory(&m.reader, od, 0, key).unwrap();
        let c2 = unpack_llcategory(&m.reader, od, 0, key).unwrap();
        s.emit(
            &format!("c9.category.{}", x8(value)),
            "category",
            &[packet_hex(&bytes), x8(c1.as_u32()), x8(c2.as_u32())],
        );
    }
    // Sale info: unpackMessage and unpackMultiMessage over (1, 77), then
    // packMessage of the raw values.
    for st in [0u8, 1, 2, 3, 4, 200, 255] {
        for p in [-5, 0, 10, i32::MAX, i32::MIN] {
            let cs = format!("c9.sale.{st}.{p}");
            let bytes = object_properties(
                m,
                Middle::Raw {
                    agg: 0,
                    category: 0,
                    sale_type: st,
                    price: p,
                },
            );
            m.dispatch("ObjectProperties", &bytes);
            let mut s1 = SaleInfo {
                sale_type: SaleType(1),
                price: 77,
            };
            let mut s2 = s1;
            unpack_sale(&m.reader, od, 0, &sale_keys, &mut s1).unwrap();
            unpack_sale(&m.reader, od, 0, &sale_keys, &mut s2).unwrap();
            let mut cols = sale_cols(&s1);
            cols.extend(sale_cols(&s2));
            s.emit(&cs, "sale", &cols);
            let raw = SaleInfo {
                sale_type: SaleType(i32::from(st)),
                price: p,
            };
            let custom = |m: &mut Messages, t: &Template| {
                pack_sale(&mut m.builder, &sale_keys, &raw).unwrap();
                m.add(t, "ObjectData", "AggregatePerms", &[0], VarType::U8);
                m.add(
                    t,
                    "ObjectData",
                    "Category",
                    &0u32.to_le_bytes(),
                    VarType::U32,
                );
            };
            let packed = object_properties(m, Middle::Custom(&custom));
            s.emit(&cs, "packed", &[packet_hex(&packed)]);
        }
    }
}

/// The unpacked CRC verdicts the README states: a block packed from an item
/// with a last owner fails the check; only `c5.perm.default` passes.
#[test]
fn c9_crc_verdicts() {
    let lines = c9_item_lines().0;
    let passing: Vec<&str> = lines
        .iter()
        .filter(|l| l.starts_with("c5.") && l.contains("\tunpack.fresh\t1\t"))
        .map(|l| l.split('\t').next().unwrap())
        .collect();
    assert_eq!(passing, ["c5.perm.default"]);
}
