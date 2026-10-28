//! R4a set 4: the messages the viewer sends (`fixtures/task/README.md`,
//! "Set 4").
//!
//! - `messages.txt`: 41 cases, each set up without capture (the fake xfer
//!   manager), then an action captured: the state before and after, the
//!   side effects, every datagram (the crate's [`Outgoing`] records packed
//!   with `vkstorm_inventory::task::message::pack` and built, reliable and
//!   zero-coded, with `vkstorm-messages`; `RequestXfer` and `AbortXfer` from
//!   the harness's model of the real `LLXferManager`), and the entries;
//! - `create_item.txt` and `create/`: `createInventoryItem`'s POST URL and
//!   body (binary LLSD byte for byte, notation and XML), the reply and its
//!   effects, and the failures without region or capability.
//!
//! [`Outgoing`]: vkstorm_inventory::task::Outgoing

mod common;
mod r4a;

use r4a::*;
use vkstorm_inventory::llsd::coerce::insert;
use vkstorm_inventory::perms::{Permissions, NULL_ID, PERM_ALL, PERM_COPY, PERM_MODIFY, PERM_MOVE, PERM_NONE, PERM_TRANSFER};
use vkstorm_inventory::sale::SaleType;
use vkstorm_inventory::task::{self, CreateRequest, Entry, TASK_INVENTORY_ASSET_KEY, TASK_INVENTORY_ITEM_KEY};
use vkstorm_inventory::types::{AssetType, InventoryType};
use vkstorm_llsd::Value;

/// `preload`: the standard contents (or `file`) through the UDP path with
/// the fake xfer manager, nothing captured.
fn preload_with(s: &mut Scene, file: Option<&[u8]>) {
    let capture = s.capture.take();
    s.request_inventory();
    let serial = s.obj.st.expected;
    s.deliver_reply(serial, b"inv_preload.tmp");
    if let Some(i) = s.live_xfer(false) {
        let x = s.take_xfer(i);
        s.write_file(&x.local, file.unwrap_or(standard_file()));
        s.xfer_callback(&x.ft, 0);
    }
    s.take();
    s.capture = capture;
}

fn preload(s: &mut Scene) {
    preload_with(s, None);
}

fn none(_: &mut Scene) {}

fn select(s: &mut Scene) {
    preload(s);
    s.apply("select");
}

fn notowner(s: &mut Scene) {
    preload(s);
    s.obj.you_owner = false;
}

/// The old entry of `spec`'s id replaced by a group-owned copy.
fn make_group_owned(s: &mut Scene, old: ItemSpec) {
    let g = ItemSpec { group: other_group(), group_owned: true, owner: NULL_ID, ..old };
    s.delete_inventory_item(&g.id);
    s.obj.inv.as_mut().unwrap().insert(0, Entry::Item(make_item(&g)));
    s.take();
}

fn duplicate_file() -> Vec<u8> {
    let dup = ItemSpec { name: b"Duplicate A".to_vec(), asset: asset('e'), ..spec_a() };
    let mut f = legacy_object(&obj_id(), b"Contents", AssetType::CATEGORY, &NULL_ID);
    f.extend(legacy_item(&spec_a()));
    f.extend(legacy_item(&dup));
    f.extend(legacy_item(&spec_b()));
    f
}

fn rich() -> ItemSpec {
    ItemSpec {
        id: item('a'),
        name: b"Rich item with a name longer than sixty-three characters, cut by the setter".to_vec(),
        desc: b"Desc \xc3\xa9 with | pipe".to_vec(),
        txn: txn(),
        flags: 0xffff_ffff,
        date: 3_000_000_000,
        sale_type: SaleType::ORIGINAL,
        sale_price: 1234,
        group: other_group(),
        base: 0x7fff_ffff,
        owner_mask: PERM_ALL & !PERM_MODIFY,
        group_mask: PERM_COPY,
        everyone: PERM_MOVE,
        next: PERM_ALL & !PERM_TRANSFER,
        ..spec_a()
    }
}

fn script(id: char, asset_id: Option<char>, name: &[u8]) -> ItemSpec {
    ItemSpec {
        id: item(id),
        asset: asset_id.map_or(NULL_ID, asset),
        ty: AssetType::LSL_TEXT,
        inv_type: InventoryType::LSL,
        name: name.to_vec(),
        ..spec_a()
    }
}

type Step = fn(&mut Scene);

/// `message_cases()`: label, setup (not captured), action (captured),
/// capability.
fn message_cases() -> Vec<(&'static str, Step, Step, bool)> {
    vec![
        ("update_key0_new", preload, |s| {
            let d = ItemSpec { id: item('d'), asset: asset('d'), name: b"New D".to_vec(), ..spec_a() };
            s.update_item(&d, TASK_INVENTORY_ITEM_KEY, true);
        }, false),
        ("update_key0_selected", select, |s| {
            s.update_item(&ItemSpec { desc: b"edited".to_vec(), ..spec_a() }, TASK_INVENTORY_ITEM_KEY, false);
        }, false),
        ("update_key0_group_owned", |s| {
            preload(s);
            make_group_owned(s, spec_a());
        }, |s| {
            s.update_item(&ItemSpec { desc: b"edited".to_vec(), ..spec_a() }, TASK_INVENTORY_ITEM_KEY, false);
        }, false),
        ("update_key0_owner_known", preload, |s| {
            s.update_item(&ItemSpec { desc: b"edited".to_vec(), owner: other(), ..spec_a() }, TASK_INVENTORY_ITEM_KEY, false);
        }, false),
        ("update_key0_guess_owner", preload, |s| {
            let c = ItemSpec { id: item('c'), asset: asset('c'), group: other_group(), ..spec_a() };
            s.update_item(&c, TASK_INVENTORY_ITEM_KEY, false);
        }, false),
        ("update_key0_guess_not_owner", notowner, |s| {
            s.update_item(&ItemSpec { id: item('c'), asset: asset('c'), ..spec_a() }, TASK_INVENTORY_ITEM_KEY, false);
        }, false),
        ("update_key0_object_type_not_atomic", preload, |s| {
            s.update_item(&ItemSpec { id: item('c'), ..spec_b() }, TASK_INVENTORY_ITEM_KEY, false);
        }, false),
        ("update_key1_by_asset_known", preload, |s| {
            s.update_item(&ItemSpec { id: item('e'), desc: b"by asset".to_vec(), ..spec_a() }, TASK_INVENTORY_ASSET_KEY, false);
        }, false),
        ("update_key1_by_asset_guess", preload, |s| {
            s.update_item(&ItemSpec { id: item('e'), asset: asset('e'), ..spec_a() }, TASK_INVENTORY_ASSET_KEY, false);
        }, false),
        ("update_no_inventory", none, |s| {
            s.update_item(&spec_a(), TASK_INVENTORY_ITEM_KEY, false);
        }, false),
        ("update_rich_item", preload, |s| {
            s.update_item(&rich(), TASK_INVENTORY_ITEM_KEY, false);
        }, false),
        ("update_material_new", preload, |s| {
            let t = ItemSpec {
                id: item('d'),
                asset: asset('d'),
                ty: AssetType::TEXTURE,
                inv_type: InventoryType::TEXTURE,
                ..spec_a()
            };
            let item = make_item(&t);
            for _ in 0..2 {
                let acts = task::update_material_inventory(&s.view(), &s.env(), &item, TASK_INVENTORY_ITEM_KEY, true);
                s.run(acts);
            }
        }, false),
        ("update_selected_notecard_next_move", select, |s| {
            s.update_item(&ItemSpec { next: PERM_MOVE, desc: b"atomic".to_vec(), ..spec_a() }, TASK_INVENTORY_ITEM_KEY, false);
        }, false),
        ("update_selected_object_next_move", select, |s| {
            s.update_item(&ItemSpec { next: PERM_MOVE, desc: b"not atomic".to_vec(), ..spec_b() }, TASK_INVENTORY_ITEM_KEY, false);
        }, false),
        ("update_notowner_notecard_next_move", notowner, |s| {
            let c = ItemSpec { id: item('c'), asset: asset('c'), next: PERM_MOVE, ..spec_a() };
            s.update_item(&c, TASK_INVENTORY_ITEM_KEY, false);
        }, false),
        ("update_notowner_object_next_move", notowner, |s| {
            let c = ItemSpec { id: item('c'), asset: asset('c'), next: PERM_MOVE, ..spec_b() };
            s.update_item(&c, TASK_INVENTORY_ITEM_KEY, false);
        }, false),
        ("update_groupowned_notecard_next_move", |s| {
            preload(s);
            make_group_owned(s, spec_a());
        }, |s| {
            s.update_item(&ItemSpec { next: PERM_MOVE, desc: b"atomic".to_vec(), ..spec_a() }, TASK_INVENTORY_ITEM_KEY, false);
        }, false),
        ("update_groupowned_object_next_move", |s| {
            preload(s);
            make_group_owned(s, spec_b());
        }, |s| {
            s.update_item(&ItemSpec { next: PERM_MOVE, desc: b"not atomic".to_vec(), ..spec_b() }, TASK_INVENTORY_ITEM_KEY, false);
        }, false),
        ("update_known_notecard_next_move", preload, |s| {
            let a = ItemSpec { owner: other(), next: PERM_MOVE, desc: b"atomic".to_vec(), ..spec_a() };
            s.update_item(&a, TASK_INVENTORY_ITEM_KEY, false);
        }, false),
        ("update_known_object_next_move", preload, |s| {
            let b = ItemSpec { owner: other(), next: PERM_MOVE, desc: b"not atomic".to_vec(), ..spec_b() };
            s.update_item(&b, TASK_INVENTORY_ITEM_KEY, false);
        }, false),
        ("update_selected_script_next_move", select, |s| {
            let sc = ItemSpec { next: PERM_MOVE, ..script('c', Some('c'), b"Script C") };
            s.update_item(&sc, TASK_INVENTORY_ITEM_KEY, false);
        }, false),
        ("delete_duplicate_id_direct", |s| preload_with(s, Some(&duplicate_file())), |s| s.delete_inventory_item(&item('a')), false),
        ("remove_duplicate_id", |s| preload_with(s, Some(&duplicate_file())), |s| s.remove_inventory(&item('a')), false),
        ("update_local_known", preload, |s| {
            let a = make_item(&ItemSpec { desc: b"local".to_vec(), txn: txn(), ..spec_a() });
            let acts = task::update_inventory_local(&s.view(), &s.env(), &a.item, TASK_INVENTORY_ITEM_KEY);
            s.run(acts);
        }, false),
        ("update_viewer_asset", preload, |s| {
            let a = make_item(&spec_a());
            let acts = task::update_viewer_inventory_asset(&s.view(), &s.env(), &a, &asset('e'));
            s.run(acts);
        }, false),
        ("remove_present", preload, |s| s.remove_inventory(&item('a')), false),
        ("remove_absent", preload, |s| s.remove_inventory(&item('e')), false),
        ("move_null_folder_copyable", preload, |s| s.move_inventory(&NULL_ID, &item('a')), false),
        ("move_explicit_folder_nocopy", preload, |s| s.move_inventory(&folder(), &item('b')), false),
        ("move_absent", preload, |s| s.move_inventory(&folder(), &item('e')), false),
        ("request_udp", none, |s| s.request_inventory(), false),
        ("request_udp_pending_dropped", none, |s| {
            s.request_inventory();
            s.request_inventory();
        }, false),
        ("rezscript_new_template", none, |s| {
            let sc = make_item(&ItemSpec { txn: txn(), ..script('c', None, b"New Script") });
            let acts = task::save_script(&s.view(), &s.env(), &sc, true, true, &template_item());
            s.run(acts);
        }, false),
        ("rezscript_asset_with_template", preload, |s| {
            let sc = make_item(&script('c', Some('c'), b"Script"));
            let acts = task::save_script(&s.view(), &s.env(), &sc, false, false, &template_item());
            s.run(acts);
        }, false),
        ("rezscript_template_not_new", preload, |s| {
            let sc = make_item(&script('c', None, b"Script"));
            let acts = task::save_script(&s.view(), &s.env(), &sc, true, false, &template_item());
            s.run(acts);
        }, false),
        ("rezscript_new_no_template", none, |s| {
            let sc = make_item(&script('c', None, b"New Script"));
            let acts = task::save_script(&s.view(), &s.env(), &sc, true, true, &NULL_ID);
            s.run(acts);
        }, false),
        ("xfer_request", none, |s| {
            s.request_inventory();
            s.deliver_reply(0, b"inventory_obj.tmp");
        }, false),
        ("xfer_replaced_by_newer_reply", none, |s| {
            s.request_inventory();
            s.deliver_reply(0, b"inventory_first.tmp");
            s.deliver_reply(1, b"inventory_second.tmp");
        }, false),
        ("xfer_aborted_by_delayed_fetch", none, |s| {
            s.request_inventory();
            s.deliver_reply(0, b"inventory_first.tmp");
            s.obj.st.expected = 3;
            s.deliver_reply(1, b"inventory_old.tmp");
        }, false),
        ("cap_request_url", none, |s| s.request_inventory(), true),
        ("cap_request_url_after_load", |s| {
            s.request_inventory();
            s.complete_http(Some(standard_reply(9)), 200, None);
            s.take();
        }, |s| {
            s.dirty_inventory();
            s.request_inventory();
        }, true),
    ]
}

#[test]
fn set4_messages() {
    let mut text = String::new();
    for (label, setup, action, caps) in message_cases() {
        text += &format!("case {label} {}\n", if caps { "cap" } else { "udp" });
        let mut s = Scene::new(caps, 1, false);
        setup(&mut s);
        s.take();
        let before = s.state_line();
        // Capture: the real xfer manager and the datagrams
        s.xfers = Xfers::Real { list: Vec::new(), ids: Vec::new(), next: 0x5a17_0000_0000_0001 };
        s.capture = Some(Vec::new());
        action(&mut s);
        let actions = s.take();
        text += &format!("  before {before}\n  after {} | {}\n", s.state_line(), actions);
        if s.pending_http_count() > 0 {
            let (method, url, _) = s.pending_http(0);
            text += &format!("  pending {method} {url}\n");
        }
        let packets = s.capture.take().unwrap();
        text += &packets_text(&packets, &s.real_ids());
        text += &s.entries_text("  ");
    }
    assert_text("messages.txt", text.as_bytes());
}

struct CreateCase {
    label: &'static str,
    region: bool,
    cap: bool,
    asset_type: AssetType,
    inv_type: InventoryType,
    sub_type: u8,
    name: &'static [u8],
    desc: &'static [u8],
    /// base, owner, everyone, group, next
    masks: [u32; 5],
    params: Value,
    status: i32,
    success: bool,
}

fn create_cases() -> Vec<CreateCase> {
    let note = |label, region, cap, name: &'static [u8], params, status, success| CreateCase {
        label,
        region,
        cap,
        asset_type: AssetType::NOTECARD,
        inv_type: InventoryType::NOTECARD,
        sub_type: 0,
        name,
        desc: b"",
        masks: [PERM_ALL, PERM_ALL, PERM_NONE, PERM_NONE, PERM_ALL],
        params,
        status,
        success,
    };
    let lua_params = map(vec![("language", s("lua")), ("template", Value::Uuid(template_item()))]);
    vec![
        CreateCase {
            label: "script",
            region: true,
            cap: true,
            asset_type: AssetType::LSL_TEXT,
            inv_type: InventoryType::LSL,
            sub_type: 0,
            name: b"New Script",
            desc: b"",
            masks: [PERM_ALL, PERM_ALL, PERM_NONE, PERM_NONE, PERM_MOVE | PERM_TRANSFER],
            params: Value::Undefined,
            status: 200,
            success: true,
        },
        CreateCase {
            label: "lua_script_params",
            region: true,
            cap: true,
            asset_type: AssetType::LSL_TEXT,
            inv_type: InventoryType::LSL,
            sub_type: 1,
            name: b"Lua \xc3\xa9",
            desc: b"d|e",
            masks: [0xffff_ffff, 0x7fff_ffff, PERM_COPY, PERM_MOVE, 0x8000_0000],
            params: lua_params,
            status: 200,
            success: true,
        },
        CreateCase { desc: b"desc", ..note("notecard", true, true, b"New Note", Value::map(), 200, true) },
        note("server_says_no", true, true, b"N", Value::Undefined, 200, false),
        note("http_error", true, true, b"N", Value::Undefined, 500, true),
        note("no_cap", true, false, b"N", Value::Undefined, 200, true),
        note("no_region", false, true, b"N", Value::Undefined, 200, true),
    ]
}

#[test]
fn set4_create_item() {
    let mut text = String::new();
    let mut files = 0;
    for cc in create_cases() {
        let mut s = Scene::new(true, 1, false);
        s.cap_create = cc.cap;
        s.request_inventory();
        s.complete_http(Some(standard_reply(0)), 200, None);
        s.take();
        s.region = cc.region;
        text += &format!("case {}\n  before {}\n", cc.label, s.state_line());
        let mut perm = Permissions::DEFAULT;
        perm.init(&agent(), &agent(), &NULL_ID, &NULL_ID);
        let m = cc.masks;
        perm.init_masks(m[0], m[1], m[2], m[3], m[4]);
        text += &format!("  permissions {}\n", perm_text(&perm));
        let before = s.pending_http_count();
        let req = CreateRequest {
            asset_type: cc.asset_type,
            inventory_type: cc.inv_type,
            sub_type: cc.sub_type,
            name: cc.name,
            description: cc.desc,
            permissions: &perm,
            params: Some(&cc.params),
        };
        s.create_inventory_item(&req, CreateCallback::Body);
        text += &format!("  call | {}\n", s.take());
        if s.pending_http_count() > before {
            let (_, url, body) = s.pending_http(before);
            let (url, body) = (url.to_owned(), body.clone());
            let path = format!("create/{}.llsd", cc.label);
            assert_bytes(&path, &binary_llsd(&body));
            files += 1;
            let mut xml = xml_llsd(&body);
            while matches!(xml.last(), Some(b'\n' | b'\r')) {
                xml.pop();
            }
            text += &format!(
                "  post {url}\n  body {}\n  body_xml {}\n  body_file {path}\n",
                notation(&body),
                String::from_utf8(xml).unwrap()
            );
            let mut reply = Value::map();
            insert(&mut reply, "success", Value::Boolean(cc.success));
            s.complete_http(Some(reply), cc.status, Some("POST"));
        }
        text += &format!("  after {} | {}\n", s.state_line(), s.take());
    }
    assert_text("create_item.txt", text.as_bytes());
    assert_eq!(std::fs::read_dir(fixture_path("create")).unwrap().count(), files);
}
