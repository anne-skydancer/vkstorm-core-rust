//! R4b giving: regenerates `fixtures/give/{give_item, give_folder,
//! predicates, will_accept, will_accept_sweep.*, world_to_agent, share,
//! drag_give, pack_sweep.*, folder_sweep.*, predicate_sweep.*}` from the
//! crate's give decisions (`vkstorm_inventory::exchange::give`) run against
//! the harness's fakes, as `llinventory_offer_oracle.cpp` records them from
//! the frozen C++. Notes are neither compared nor produced.

mod r4b_common;

use std::collections::BTreeSet;

use r4b_common::*;
use vkstorm_inventory::exchange::give::{self, Cargo, DragSource, DragState, Functor};
use vkstorm_inventory::exchange::host::{Action, Host, PackArgs, TaskObject, ViewerObject, LL_PCODE_VOLUME};
use vkstorm_inventory::exchange::im::*;
use vkstorm_inventory::llsd::coerce::insert;
use vkstorm_inventory::perms::{Permissions, Uuid, NULL_ID, PERM_ALL, PERM_COPY, PERM_NONE, PERM_TRANSFER};
use vkstorm_inventory::types::AssetType;
use vkstorm_llsd::Value;

const NAMES: [&str; 8] = [
    "ACCEPT_POSTPONED",
    "ACCEPT_NO",
    "ACCEPT_NO_CUSTOM",
    "ACCEPT_NO_LOCKED",
    "ACCEPT_YES_COPY_SINGLE",
    "ACCEPT_YES_SINGLE",
    "ACCEPT_YES_COPY_MULTI",
    "ACCEPT_YES_MULTI",
];

fn full() -> Permissions {
    perms_simple(&agent(), perm_mask(true, true, true))
}

// ================================================================ items

struct GiveItemCase {
    id: String,
    build: Box<dyn Fn(&mut Fake) -> Uuid>,
    session: Uuid,
    respond: i32,
    between: Option<fn(&mut Fake)>,
}

fn item_case(id: &str, build: impl Fn(&mut Fake) -> Uuid + 'static) -> GiveItemCase {
    GiveItemCase { id: id.to_owned(), build: Box::new(build), session: NULL_ID, respond: -1, between: None }
}

#[test]
fn give_item() {
    let mut cases = Vec::new();
    let nocopy = || perms_simple(&agent(), perm_mask(false, true, true));
    let notrans = || perms_simple(&agent(), perm_mask(true, true, false));
    for t in [0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 13, 17, 18, 19, 20, 21, 22, 24, 25, 26, 40, 45, 49, 56, 57, 58, 59] {
        cases.push(item_case(&format!("type_{t:02}"), move |f| f.add_item(uid(0x11, 1), root(), t, &full(), b"Gift Item", None).id));
    }
    for t in [5, 6, 13, 21] {
        cases.push(item_case(&format!("worn_type_{t:02}"), move |f| {
            let id = uid(0x11, 1);
            f.add_item(id, root(), t, &full(), b"Worn Item", None);
            match t {
                6 => f.attachments.insert(id),
                21 => f.gestures.insert(id),
                _ => f.wearables.insert(id),
            };
            id
        }));
    }
    cases.push(item_case("link_to_notecard", |f| {
        f.add_item(uid(0x12, 1), root(), 7, &full(), b"Target Notecard", None);
        f.add_item(uid(0x11, 1), root(), 24, &full(), b"Link", Some(uid(0x12, 1))).id
    }));
    cases.push(item_case("link_to_worn_object", |f| {
        f.add_item(uid(0x12, 1), root(), 6, &full(), b"Target Object", None);
        f.attachments.insert(uid(0x12, 1));
        f.add_item(uid(0x11, 1), root(), 24, &full(), b"Link", Some(uid(0x12, 1))).id
    }));
    cases.push(item_case("link_to_link", |f| {
        f.add_item(uid(0x13, 1), root(), 7, &full(), b"Deep Target", None);
        f.add_item(uid(0x12, 1), root(), 24, &full(), b"Inner Link", Some(uid(0x13, 1)));
        f.add_item(uid(0x11, 1), root(), 24, &full(), b"Outer Link", Some(uid(0x12, 1))).id
    }));
    cases.push(item_case("link_broken", |f| f.add_item(uid(0x11, 1), root(), 24, &full(), b"Broken Link", Some(uid(0x12, 9))).id));
    cases.push(item_case("folder_link", |f| {
        f.add_cat(uid(0x14, 1), root(), b"Linked Folder", -1);
        f.add_item(uid(0x11, 1), root(), 25, &full(), b"Folder Link", Some(uid(0x14, 1))).id
    }));
    cases.push(item_case("folder_link_broken", |f| f.add_item(uid(0x11, 1), root(), 25, &full(), b"Folder Link", Some(uid(0x14, 9))).id));
    cases.push(item_case("link_nocopy_link_perms", move |f| {
        f.add_item(uid(0x12, 1), root(), 7, &perms_simple(&agent(), perm_mask(true, true, true)), b"Target", None);
        f.add_item(uid(0x11, 1), root(), 24, &nocopy(), b"Link", Some(uid(0x12, 1))).id
    }));
    let names: Vec<(&str, Vec<u8>, bool)> = vec![
        ("name_empty", b"".to_vec(), false),
        ("name_corrected", b"a|b\x01c\xc3\xa9  ".to_vec(), false),
        ("name_62", vec![b'n'; 62], false),
        ("name_63", vec![b'n'; 63], false),
        ("name_64_corrected", vec![b'n'; 64], false),
        ("name_raw_1182", vec![b'x'; 1182], true),
        ("name_raw_1183", vec![b'x'; 1183], true),
        ("name_raw_1184", vec![b'x'; 1184], true),
        ("name_raw_1198", vec![b'x'; 1198], true),
        ("name_raw_1199", vec![b'x'; 1199], true),
        ("name_raw_1200", vec![b'x'; 1200], true),
        ("name_raw_1300", vec![b'x'; 1300], true),
        ("name_raw_embedded_nul", b"abc\0def".to_vec(), true),
        ("name_raw_leading_nul", b"\0abc".to_vec(), true),
    ];
    for (id, name, raw) in names {
        cases.push(item_case(id, move |f| {
            let it = f.add_item(uid(0x11, 1), root(), 7, &full(), if raw { b"placeholder" } else { &name }, None);
            if raw {
                f.set_item_raw_name(&it.id, &name);
            }
            it.id
        }));
    }
    cases.push(item_case("not_transferable", move |f| f.add_item(uid(0x11, 1), root(), 7, &notrans(), b"Kept", None).id));
    cases.push(item_case("not_owner_everyone_copy", |f| {
        f.add_item(uid(0x11, 1), root(), 7, &make_perms(&other2(), PERM_ALL, PERM_COPY, PERM_ALL, &NULL_ID, false), b"Not Mine", None).id
    }));
    cases.push(item_case("null_item", |_| NULL_ID));
    cases.push(item_case("avatar_invalid", |f| {
        f.avatar_valid = false;
        f.add_item(uid(0x11, 1), root(), 7, &full(), b"Gift", None).id
    }));
    let mut sess = item_case("im_session", |f| f.add_item(uid(0x11, 1), root(), 7, &full(), b"Gift", None).id);
    sess.session = uid(0x5e, 1);
    cases.push(sess);
    cases.push(item_case("recipient_muted", |f| {
        f.mute_insert(other(), b"other.sender", 1, 0);
        f.add_item(uid(0x11, 1), root(), 7, &full(), b"Gift", None).id
    }));
    cases.push(item_case("recipient_muted_by_name_only", |f| {
        f.legacy_mutes.insert(b"other.sender".to_vec());
        f.add_item(uid(0x11, 1), root(), 7, &full(), b"Gift", None).id
    }));
    cases.push(item_case("rlv_share_blocked", |f| {
        f.rlv_enabled = true;
        f.rlv_share_blocked.insert(other());
        f.add_item(uid(0x11, 1), root(), 7, &full(), b"Gift", None).id
    }));
    cases.push(item_case("rlv_names_hidden_nearby", |f| {
        f.rlv_enabled = true;
        f.rlv_can_show_names = false;
        f.rlv_nearby.insert(other());
        f.add_item(uid(0x11, 1), root(), 7, &full(), b"Gift", None).id
    }));
    cases.push(item_case("open_im_session_logs_there", |f| {
        f.im_sessions.insert(give::compute_session_id(&agent(), &other()));
        f.add_item(uid(0x11, 1), root(), 7, &full(), b"Gift", None).id
    }));
    cases.push(item_case("no_particle_effect", |f| {
        f.settings_b.insert("FSCreateGiveInventoryParticleEffect".into(), false);
        f.add_item(uid(0x11, 1), root(), 7, &full(), b"Gift", None).id
    }));
    let nc = |id: &str, respond: i32, session: Uuid, between: Option<fn(&mut Fake)>| GiveItemCase {
        id: id.to_owned(),
        build: Box::new(move |f: &mut Fake| f.add_item(uid(0x11, 1), root(), 7, &nocopy(), b"No Copy", None).id),
        session,
        respond,
        between,
    };
    cases.push(nc("nocopy_warning_only", -1, NULL_ID, None));
    cases.push(nc("nocopy_yes", 0, NULL_ID, None));
    cases.push(nc("nocopy_no", 1, NULL_ID, None));
    cases.push(nc("nocopy_yes_item_gone", 0, NULL_ID, Some(|f| f.act(Action::DeleteObject(uid(0x11, 1))))));
    cases.push(nc(
        "nocopy_yes_rlv_share_blocked",
        0,
        NULL_ID,
        Some(|f| {
            f.rlv_enabled = true;
            f.rlv_share_blocked.insert(other());
        }),
    ));
    cases.push(nc("nocopy_yes_avatar_invalid", 0, NULL_ID, Some(|f| f.avatar_valid = false)));
    cases.push(nc("nocopy_yes_session_lost", 0, uid(0x5e, 1), None));

    let mut out = Out::default();
    for c in &cases {
        let mut f = setup();
        let id = (c.build)(&mut f);
        f.ev.clear();
        put_world(&mut out, &f, &c.id);
        f.add(format!("give item {} to {} session {}", us(&id), us(&other()), us(&c.session)));
        let item = f.item(&id);
        let r = give::give_item(&mut f, &other(), item.as_ref(), &c.session);
        f.add(format!("result {}", r as i32));
        if c.respond >= 0 {
            if let Some(b) = c.between {
                b(&mut f);
            }
            f.respond_pending(c.respond);
        }
        f.run_idle();
        out.events(&mut f, &c.id);
    }
    assert_lines("give/give_item", &compared("give", "give_item.txt"), &out.lines);
}

// ============================================================== folders

#[derive(Clone)]
struct Node {
    is_cat: bool,
    id: Uuid,
    name: Vec<u8>,
    ty: i32,
    copy: bool,
    transfer: bool,
    worn: i32,
    incomplete: bool,
    link_target: Option<Uuid>,
    children: Vec<Node>,
}

struct Ids(u64);

impl Ids {
    fn cat(&mut self, name: &[u8], children: Vec<Node>, incomplete: bool) -> Node {
        self.0 += 1;
        Node {
            is_cat: true,
            id: uid(0x2c, self.0),
            name: name.to_vec(),
            ty: 7,
            copy: true,
            transfer: true,
            worn: 0,
            incomplete,
            link_target: None,
            children,
        }
    }
    fn item(&mut self, name: &[u8], ty: i32, copy: bool, transfer: bool, worn: i32) -> Node {
        self.0 += 1;
        Node {
            is_cat: false,
            id: uid(0x21, self.0),
            name: name.to_vec(),
            ty,
            copy,
            transfer,
            worn,
            incomplete: false,
            link_target: None,
            children: Vec::new(),
        }
    }
    fn many(&mut self, n: usize, prefix: &str, copy: bool) -> Vec<Node> {
        (0..n).map(|i| self.item(format!("{prefix} {i:02}").as_bytes(), if i % 3 == 1 { 0 } else { 7 }, copy, true, 0)).collect()
    }
}

fn build(f: &mut Fake, n: &Node, parent: Uuid) {
    if n.is_cat {
        f.add_cat(n.id, parent, &n.name, -1);
        if n.incomplete {
            f.incomplete.insert(n.id);
        }
        for c in &n.children {
            build(f, c, n.id);
        }
    } else {
        let it = f.add_item(n.id, parent, n.ty, &perms_simple(&agent(), perm_mask(n.copy, true, n.transfer)), &n.name, n.link_target);
        if n.worn == 1 {
            f.attachments.insert(it.id);
        }
        if n.worn == 2 {
            f.wearables.insert(it.id);
        }
    }
}

type Tree = fn(&mut Ids) -> Node;
type Hook = fn(&mut Fake, &Node);

struct FolderCase {
    id: String,
    tree: Tree,
    notification: Vec<u8>,
    respond: i32,
    between: Option<Hook>,
    before: Option<Hook>,
    raw_name: Option<Vec<u8>>,
    limit: i32,
}

fn fc(id: &str, tree: Tree) -> FolderCase {
    FolderCase { id: id.to_owned(), tree, notification: Vec::new(), respond: -1, between: None, before: None, raw_name: None, limit: 42 }
}

fn give_folder_case(out: &mut Out, c: &FolderCase) {
    let mut f = setup();
    let mut ids = Ids(0);
    let root_node = (c.tree)(&mut ids);
    build(&mut f, &root_node, root());
    if let Some(n) = &c.raw_name {
        f.set_cat_raw_name(&root_node.id, n);
    }
    f.max_transfer = c.limit;
    if let Some(b) = c.before {
        b(&mut f, &root_node);
    }
    f.ev.clear();
    put_world(out, &f, &c.id);
    f.add(format!(
        "give folder {} to {} limit {} notification \"{}\"",
        us(&root_node.id),
        us(&other()),
        c.limit,
        String::from_utf8_lossy(&c.notification)
    ));
    let cat = f.category(&root_node.id);
    let r = give::give_category(&mut f, &other(), cat.as_ref(), &NULL_ID, &c.notification);
    f.add(format!("result {}", r as i32));
    if c.respond >= 0 {
        if let Some(b) = c.between {
            b(&mut f, &root_node);
        }
        f.respond_pending(c.respond);
    }
    f.run_idle();
    out.events(&mut f, &c.id);
}

#[test]
fn give_folder() {
    let mut cases = vec![
        fc("empty", |i| i.cat(b"Empty", vec![], false)),
        fc("one_item", |i| {
            let a = i.item(b"Only", 7, true, true, 0);
            i.cat(b"One", vec![a], false)
        }),
        fc("flat_mixed_types", |i| {
            let v = vec![
                i.item(b"Note", 7, true, true, 0),
                i.item(b"Tex", 0, true, true, 0),
                i.item(b"Obj", 6, true, true, 0),
                i.item(b"Script", 10, true, true, 0),
                i.item(b"Anim", 20, true, true, 0),
            ];
            i.cat(b"Flat", v, false)
        }),
        fc("nested", |i| {
            let i1 = i.item(b"i1", 7, true, true, 0);
            let a1 = i.cat(b"A1", vec![i1], false);
            let i2 = i.item(b"i2", 7, true, true, 0);
            let a = i.cat(b"A", vec![a1, i2], false);
            let b = i.cat(b"B", vec![], false);
            let i3 = i.item(b"i3", 7, true, true, 0);
            let i4 = i.item(b"i4", 7, true, true, 0);
            i.cat(b"Root", vec![a, b, i3, i4], false)
        }),
        fc("deep_4", |i| {
            let deep = i.item(b"deep", 7, true, true, 0);
            let d3 = i.cat(b"D3", vec![deep], false);
            let d2 = i.cat(b"D2", vec![d3], false);
            let d1 = i.cat(b"D1", vec![d2], false);
            let top = i.item(b"top", 7, true, true, 0);
            i.cat(b"D0", vec![d1, top], false)
        }),
    ];
    // two empty sub-folders in front of 39, 40 or 41 items (the items get
    // their ids first, as the C++ builds the vector before inserting)
    let entries = |id: &str, n: usize| -> FolderCase {
        let tree: Tree = match n {
            39 => |i| {
                let mut v = i.many(39, "item", true);
                let s1 = i.cat(b"S1", vec![], false);
                v.insert(0, s1);
                let s2 = i.cat(b"S2", vec![], false);
                v.insert(0, s2);
                i.cat(b"Forty One", v, false)
            },
            40 => |i| {
                let mut v = i.many(40, "item", true);
                let s1 = i.cat(b"S1", vec![], false);
                v.insert(0, s1);
                let s2 = i.cat(b"S2", vec![], false);
                v.insert(0, s2);
                i.cat(b"Forty Two", v, false)
            },
            _ => |i| {
                let mut v = i.many(41, "item", true);
                let s1 = i.cat(b"S1", vec![], false);
                v.insert(0, s1);
                let s2 = i.cat(b"S2", vec![], false);
                v.insert(0, s2);
                i.cat(b"Forty Three", v, false)
            },
        };
        fc(id, tree)
    };
    cases.push(entries("entries_41", 39));
    cases.push(entries("entries_42", 40));
    cases.push(entries("entries_43", 41));
    let big = |id: &str, tree: Tree| {
        let mut c = fc(id, tree);
        c.limit = 100;
        c
    };
    cases.push(big("opensim_limit_100_entries_70", |i| {
        let v = i.many(70, "item", true);
        i.cat(b"Big", v, false)
    }));
    cases.push(big("opensim_limit_100_entries_99", |i| {
        let v = i.many(99, "item", true);
        i.cat(b"Big99", v, false)
    }));
    cases.push(big("opensim_limit_100_entries_100", |i| {
        let v = i.many(100, "item", true);
        i.cat(b"Bigger", v, false)
    }));
    cases.push(big("opensim_limit_100_entries_101", |i| {
        let v = i.many(101, "item", true);
        i.cat(b"Too Big", v, false)
    }));
    let raw = |id: &str, tree: Tree, name: Vec<u8>| {
        let mut c = fc(id, tree);
        c.raw_name = Some(name);
        c
    };
    let thirty: Tree = |i| {
        let v = i.many(30, "item", true);
        i.cat(b"x", v, false)
    };
    let one: Tree = |i| {
        let v = i.many(1, "item", true);
        i.cat(b"x", v, false)
    };
    cases.push(raw("name_raw_700_entries_30", thirty, vec![b'L'; 700]));
    cases.push(raw("name_raw_1198_entries_1", one, vec![b'L'; 1198]));
    cases.push(raw("name_raw_1200_entries_1", one, vec![b'L'; 1200]));
    cases.push(raw("name_raw_1199_entries_1", one, vec![b'L'; 1199]));
    cases.push(raw("name_raw_1300_entries_1", one, vec![b'L'; 1300]));
    cases.push(raw("name_raw_embedded_nul", one, b"Fol\0der".to_vec()));
    cases.push(raw("name_raw_empty", one, Vec::new()));
    cases.push(fc("name_63_corrected_max", |i| {
        let v = i.many(42, "item", true);
        i.cat(&[b'N'; 70], v, false)
    }));
    cases.push(fc("excluded_worn_and_no_transfer", |i| {
        let v = vec![
            i.item(b"worn obj", 6, true, true, 1),
            i.item(b"worn shirt", 5, true, true, 2),
            i.item(b"no transfer", 7, true, false, 0),
            i.item(b"plain", 7, true, true, 0),
        ];
        i.cat(b"Mixed", v, false)
    }));
    cases.push(fc("only_excluded", |i| {
        let v = vec![i.item(b"worn obj", 6, true, true, 1), i.item(b"no transfer", 7, true, false, 0)];
        i.cat(b"Nothing", v, false)
    }));
    cases.push(fc("links_inside", |i| {
        let target = i.item(b"target", 6, true, true, 0);
        let mut root_node = i.cat(b"Links", vec![target], false);
        let mut l = i.item(b"link", 24, true, true, 0);
        l.link_target = Some(root_node.children[0].id);
        let mut fl = i.item(b"folder link", 25, true, true, 0);
        fl.link_target = Some(uid(0x2c, 999));
        root_node.children.push(l);
        root_node.children.push(fl);
        root_node
    }));
    cases.push(fc("incomplete_subfolder", |i| {
        let p = i.cat(b"Pending", vec![], true);
        let it = i.item(b"i", 7, true, true, 0);
        i.cat(b"Inc", vec![p, it], false)
    }));
    cases.push(fc("incomplete_and_too_many", |i| {
        let mut v = i.many(43, "item", true);
        let p = i.cat(b"Pending", vec![], true);
        v.insert(0, p);
        i.cat(b"Inc", v, false)
    }));
    cases.push(fc("incomplete_and_empty", |i| {
        let p = i.cat(b"Pending", vec![], true);
        i.cat(b"Inc", vec![p], false)
    }));
    let mut trash = fc("give_trash_itself", |i| {
        let j = i.item(b"junk", 7, true, true, 0);
        i.cat(b"Trash", vec![j], false)
    });
    trash.before = Some(|f, r| {
        f.system_folders.insert(14, r.id);
    });
    cases.push(trash);
    let nc_tree: Tree = |i| {
        let a = i.item(b"copy a", 7, true, true, 0);
        let b = i.item(b"nocopy b", 7, false, true, 0);
        let c = i.item(b"nocopy c", 7, false, true, 0);
        let d = i.item(b"copy d", 7, true, true, 0);
        let sub = i.cat(b"Sub", vec![c, d], false);
        i.cat(b"NC", vec![a, b, sub], false)
    };
    let nc = |id: &str, respond: i32, notification: &[u8], between: Option<Hook>| {
        let mut c = fc(id, nc_tree);
        c.respond = respond;
        c.notification = notification.to_vec();
        c.between = between;
        c
    };
    cases.push(nc("nocopy_warning_only", -1, b"", None));
    cases.push(nc("nocopy_yes", 0, b"", None));
    cases.push(nc("nocopy_no", 1, b"", None));
    cases.push(nc("nocopy_yes_success_notification", 0, b"ItemsShared", None));
    cases.push(nc(
        "nocopy_yes_count_changed_too_many",
        0,
        b"",
        Some(|f, r| {
            for i in 0..40u64 {
                f.add_item(uid(0x29, i + 1), r.id, 7, &perms_simple(&agent(), perm_mask(true, true, true)), format!("late {i:02}").as_bytes(), None);
            }
        }),
    ));
    cases.push(nc(
        "nocopy_yes_folder_gone",
        0,
        b"",
        Some(|f, r| {
            f.cat_map.remove(&r.id);
        }),
    ));
    cases.push(nc(
        "nocopy_yes_rlv_share_blocked",
        0,
        b"",
        Some(|f, _| {
            f.rlv_enabled = true;
            f.rlv_share_blocked.insert(other());
        }),
    ));
    let mut ncw = fc("nocopy_worn_nocopy_not_deleted", |i| {
        let a = i.item(b"worn nocopy", 6, false, true, 1);
        let b = i.item(b"nocopy", 7, false, true, 0);
        let c = i.item(b"copy", 7, true, true, 0);
        i.cat(b"NCW", vec![a, b, c], false)
    });
    ncw.respond = 0;
    cases.push(ncw);
    let one_a: Tree = |i| {
        let a = i.item(b"a", 7, true, true, 0);
        i.cat(b"R", vec![a], false)
    };
    let mut rlv = fc("rlv_share_blocked_copyable", one_a);
    rlv.before = Some(|f, _| {
        f.rlv_enabled = true;
        f.rlv_share_blocked.insert(other());
    });
    cases.push(rlv);
    let mut muted = fc("recipient_muted", |i| {
        let a = i.item(b"a", 7, true, true, 0);
        i.cat(b"M", vec![a], false)
    });
    muted.before = Some(|f, _| f.mute_insert(other(), b"other.sender", 1, 0));
    cases.push(muted);
    let mut inv = fc("avatar_invalid", |i| {
        let a = i.item(b"a", 7, true, true, 0);
        i.cat(b"A", vec![a], false)
    });
    inv.before = Some(|f, _| f.avatar_valid = false);
    cases.push(inv);

    let mut out = Out::default();
    for c in &cases {
        give_folder_case(&mut out, c);
    }
    assert_lines("give/give_folder", &compared("give", "give_folder.txt"), &out.lines);
}

// =========================================================== predicates

fn functor_out(f: &Fake, name: &str, item: Option<&vkstorm_inventory::exchange::host::ModelItem>, cat: Option<&vkstorm_inventory::exchange::host::ModelCat>) -> (bool, i32) {
    let (r, c) = give::collect_functor(f, Functor::from_name(name).unwrap(), cat, item);
    (r, c.unwrap_or(-1))
}

#[test]
fn predicates() {
    let mut out = Out::default();
    let types: [(&str, i32, bool); 9] = [
        ("object", 6, false),
        ("bodypart", 13, false),
        ("clothing", 5, false),
        ("gesture", 21, false),
        ("notecard", 7, false),
        ("texture", 0, false),
        ("callingcard", 2, false),
        ("link_object", 6, true),
        ("link_notecard", 7, true),
    ];
    let worns = ["none", "attached", "wearable", "gesture"];
    let owners = ["agent", "other", "group"];
    {
        let mut f = setup();
        f.groups.insert(group());
        f.add_item(uid(0x32, 1), root(), 6, &perms_simple(&agent(), PERM_ALL), b"target", None);
        let lines = [
            format!(
                "item {} parent={} name=\"item\" (\"link\" when link=1, then type=24 and asset={}, the target) inv_type=default for the type flags=0 finished=1",
                us(&uid(0x31, 1)),
                us(&root()),
                us(&uid(0x32, 1))
            ),
            format!("perm creator={} last_owner=owner group={} (the agent is in the group)", us(&creator()), us(&group())),
            format!("owner kinds: agent={} other={} group=group-owned (owner null, group_owned=1)", us(&agent()), us(&other2())),
            format!("link target: {} (its type is the link's type token)", f.item_rec_line(&f.items[&uid(0x32, 1)])),
            "worn: attached = the worn id is attached; wearable = worn by gAgentWearables; gesture = an active gesture; the worn id is the target for links".to_owned(),
            "case id: <kind>.<worn>.<owner>.<owner mask: C/c copy, M/m modify, T/t transfer>.e<everyone copy>.b<f: base full, nt: base without transfer>.a<avatar valid>".to_owned(),
            "in: t=asset type l=link o=owner kind base/own/grp/evr/nxt = the masks after LLPermissions::initMasks".to_owned(),
            "out: g=isInventoryGiveAcceptable gg=isInventoryGroupGiveAcceptable tc=itemTransferCommonlyAllowed gv=LLGiveable/countNoCopy uc=LLUncopyableItems dt=LLDroppableItem(true)/countNoCopy dn=LLDroppableItem(false)/countNoCopy dc=LLDropCopyableItems".to_owned(),
        ];
        for (n, l) in lines.iter().enumerate() {
            out.put("legend", &format!("input.{n:02}"), l);
        }
    }
    let mut matrix = 0;
    for (tid, tty, link) in types {
        for worn in 0..4 {
            for owner in 0..3 {
                for mask in 0..8u32 {
                    for everyone_copy in [false, true] {
                        for base_full in [false, true] {
                            for avatar in [false, true] {
                                let mut f = setup();
                                f.groups.insert(group());
                                f.avatar_valid = avatar;
                                let (copy, modify, transfer) = (mask & 1 != 0, mask & 2 != 0, mask & 4 != 0);
                                let owner_id = [agent(), other2(), NULL_ID][owner];
                                let perm = make_perms(
                                    &owner_id,
                                    perm_mask(copy, modify, transfer),
                                    if everyone_copy { PERM_COPY } else { PERM_NONE },
                                    if base_full { PERM_ALL } else { PERM_ALL & !PERM_TRANSFER },
                                    &group(),
                                    owner == 2,
                                );
                                let item_id = uid(0x31, 1);
                                let target = uid(0x32, 1);
                                if link {
                                    f.add_item(target, root(), tty, &perms_simple(&agent(), PERM_ALL), b"target", None);
                                    f.add_item(item_id, root(), 24, &perm, b"link", Some(target));
                                } else {
                                    f.add_item(item_id, root(), tty, &perm, b"item", None);
                                }
                                let worn_id = if link { target } else { item_id };
                                match worn {
                                    1 => {
                                        f.attachments.insert(worn_id);
                                    }
                                    2 => {
                                        f.wearables.insert(worn_id);
                                    }
                                    3 => {
                                        f.gestures.insert(worn_id);
                                    }
                                    _ => {}
                                }
                                let it = f.item(&item_id).unwrap();
                                let c = format!(
                                    "{tid}.{}.{}.{}{}{}.e{}.b{}.a{}",
                                    worns[worn],
                                    owners[owner],
                                    if copy { "C" } else { "c" },
                                    if modify { "M" } else { "m" },
                                    if transfer { "T" } else { "t" },
                                    everyone_copy as i32,
                                    if base_full { "f" } else { "nt" },
                                    avatar as i32
                                );
                                let p = &it.perm;
                                out.put(
                                    &c,
                                    "in",
                                    &format!(
                                        "t={} l={} o={} base={:08x} own={:08x} grp={:08x} evr={:08x} nxt={:08x}",
                                        it.ty.0,
                                        link as i32,
                                        owners[owner],
                                        p.base,
                                        p.owner_mask,
                                        p.group_mask,
                                        p.everyone,
                                        p.next
                                    ),
                                );
                                let (gv, gvc) = functor_out(&f, "giveable", Some(&it), None);
                                let (dt, dtc) = functor_out(&f, "droppable_transfer", Some(&it), None);
                                let (dn, dnc) = functor_out(&f, "droppable_no_transfer", Some(&it), None);
                                out.put(
                                    &c,
                                    "out",
                                    &format!(
                                        "g={} gg={} tc={} gv={}/{} uc={} dt={}/{} dn={}/{} dc={}",
                                        give::is_give_acceptable(&f, Some(&it)) as i32,
                                        give::is_group_give_acceptable(&f, Some(&it)) as i32,
                                        give::transfer_commonly_allowed(&f, Some(&it)) as i32,
                                        gv as i32,
                                        gvc,
                                        functor_out(&f, "uncopyable", Some(&it), None).0 as i32,
                                        dt as i32,
                                        dtc,
                                        dn as i32,
                                        dnc,
                                        functor_out(&f, "drop_copyable", Some(&it), None).0 as i32
                                    ),
                                );
                                matrix += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    let rules: Vec<(&str, Permissions)> = vec![
        ("rule_owner_full_base_full", make_perms(&agent(), PERM_ALL, PERM_NONE, PERM_ALL, &NULL_ID, false)),
        ("rule_owner_copy_base_nocopy", make_perms(&agent(), PERM_ALL, PERM_NONE, PERM_ALL & !PERM_COPY, &NULL_ID, false)),
        ("rule_owner_transfer_base_notransfer", make_perms(&agent(), PERM_ALL, PERM_NONE, PERM_ALL & !PERM_TRANSFER, &NULL_ID, false)),
        ("rule_next_owner_copy", {
            let mut p = perms_simple(&agent(), PERM_ALL);
            p.init_masks(PERM_ALL, PERM_ALL, PERM_NONE, PERM_NONE, PERM_ALL);
            p
        }),
        ("rule_next_owner_nocopy", {
            let mut p = perms_simple(&agent(), PERM_ALL);
            p.init_masks(PERM_ALL, PERM_ALL, PERM_NONE, PERM_NONE, PERM_ALL & !PERM_COPY);
            p
        }),
        ("rule_group_owned_atomic", {
            let mut p = make_perms(&agent(), PERM_ALL, PERM_NONE, PERM_ALL, &group(), false);
            p.set_owner_and_group(&agent(), &NULL_ID, &group(), true);
            p
        }),
        ("rule_group_owned_not_atomic", {
            let mut p = make_perms(&agent(), PERM_ALL, PERM_NONE, PERM_ALL, &group(), false);
            p.set_owner_and_group(&agent(), &NULL_ID, &group(), false);
            p
        }),
        ("rule_other_owner_everyone_copy", make_perms(&other2(), PERM_ALL, PERM_COPY, PERM_ALL, &NULL_ID, false)),
        ("rule_other_owner_everyone_none", make_perms(&other2(), PERM_ALL, PERM_NONE, PERM_ALL, &NULL_ID, false)),
    ];
    for (id, perm) in &rules {
        let mut f = setup();
        f.groups.insert(group());
        f.add_item(uid(0x31, 1), root(), 7, perm, b"item", None);
        let rec = f.items[&uid(0x31, 1)].clone();
        put_items(&mut out, &f, id, &[&rec]);
        let it = rec.rec.clone();
        let (gv, gvc) = functor_out(&f, "giveable", Some(&it), None);
        let (dt, dtc) = functor_out(&f, "droppable_transfer", Some(&it), None);
        out.put(
            id,
            "result",
            &format!(
                "give={} group_give={} giveable={}/{} uncopyable={} droppable_transfer={}/{} drop_copyable={}",
                give::is_give_acceptable(&f, Some(&it)) as i32,
                give::is_group_give_acceptable(&f, Some(&it)) as i32,
                gv as i32,
                gvc,
                functor_out(&f, "uncopyable", Some(&it), None).0 as i32,
                dt as i32,
                dtc,
                functor_out(&f, "drop_copyable", Some(&it), None).0 as i32
            ),
        );
    }
    let fnames = ["giveable", "uncopyable", "droppable_transfer", "droppable_no_transfer", "drop_copyable"];
    let mut f = setup();
    let cc = f.add_cat(uid(0x33, 1), root(), b"cat", -1);
    let parts: Vec<String> = fnames
        .iter()
        .map(|n| {
            let (r, c) = functor_out(&f, n, None, Some(&cc));
            format!("{n}={}/{c}", r as i32)
        })
        .collect();
    out.put("category_only", "result", &parts.join(" "));
    let parts: Vec<String> = fnames
        .iter()
        .map(|n| {
            let (r, c) = functor_out(&f, n, None, None);
            format!("{n}={}/{c}", r as i32)
        })
        .collect();
    out.put("null_both", "result", &parts.join(" "));
    out.put(
        "null_item",
        "result",
        &format!(
            "give={} group_give={} transfer_common={}",
            give::is_give_acceptable(&f, None) as i32,
            give::is_group_give_acceptable(&f, None) as i32,
            give::transfer_commonly_allowed(&f, None) as i32
        ),
    );
    assert_lines("give/predicates", &compared("give", "predicates.txt"), &out.lines);
    println!("give/predicates: {matrix} matrix cases, {} rules", rules.len());
}

// ========================================================== will accept

#[derive(Clone, Copy, Default)]
struct AcceptCase {
    modify: bool,
    add: bool,
    you_owner: bool,
    volume: bool,
    attachment: bool,
    item_owner_agent: bool,
    item_transfer: bool,
    unrestricted: bool,
    ty: usize,
    worn: bool,
    finished: bool,
    parent_is_object: bool,
    dad_category: bool,
    rlv: i32,
    null_obj: bool,
    null_item: bool,
}

const ACCEPT_TYPES: [i32; 5] = [6, 5, 2, 7, 24];
const ACCEPT_TYPE_NAMES: [&str; 5] = ["object", "clothing", "callingcard", "notecard", "link"];

fn accept_id(a: &AcceptCase) -> String {
    format!(
        "obj_m{}_a{}_o{}_v{}_att{}.item_own{}_t{}_u{}_{}_w{}_f{}_p{}.dad_{}.rlv{}",
        a.modify as i32,
        a.add as i32,
        a.you_owner as i32,
        a.volume as i32,
        a.attachment as i32,
        a.item_owner_agent as i32,
        a.item_transfer as i32,
        a.unrestricted as i32,
        ACCEPT_TYPE_NAMES[a.ty],
        a.worn as i32,
        a.finished as i32,
        a.parent_is_object as i32,
        if a.dad_category { "category" } else { "item" },
        a.rlv
    )
}

fn run_accept(out: &mut Out, a: &AcceptCase, cid: &str) {
    let mut f = setup();
    let obj = ViewerObject {
        id: uid(0x41, 1),
        perm_modify: a.modify,
        flag_allow_inventory_add: a.add,
        perm_you_owner: a.you_owner,
        pcode: if a.volume { LL_PCODE_VOLUME } else { 47 },
        is_attachment: a.attachment,
        root: uid(0x41, 2),
    };
    f.rlv_enabled = a.rlv != 0;
    f.rlv_locked_attachment = a.rlv == 2;
    f.rlv_unsit = a.rlv == 3;
    f.rlv_sittp = a.rlv == 4;
    f.sitting = a.rlv >= 3;
    f.avatar_root = Some(if a.rlv == 3 { uid(0x41, 2) } else { uid(0x41, 1) });
    let ty = ACCEPT_TYPES[a.ty];
    let perm = make_perms(
        &if a.item_owner_agent { agent() } else { other2() },
        perm_mask(true, true, a.item_transfer),
        PERM_NONE,
        if a.unrestricted { PERM_ALL } else { PERM_ALL & !PERM_TRANSFER },
        &NULL_ID,
        false,
    );
    if ty == 24 {
        f.add_item(uid(0x42, 2), root(), 7, &perms_simple(&agent(), PERM_ALL), b"target", None);
    }
    let parent = if a.parent_is_object { obj.id } else { root() };
    f.add_item(uid(0x42, 1), parent, ty, &perm, b"drop", if ty == 24 { Some(uid(0x42, 2)) } else { Some(NULL_ID) });
    f.items.get_mut(&uid(0x42, 1)).unwrap().rec.finished = a.finished;
    if a.worn {
        if ty == 6 {
            f.attachments.insert(uid(0x42, 1));
        } else {
            f.wearables.insert(uid(0x42, 1));
        }
    }
    let it = f.item(&uid(0x42, 1)).unwrap();
    let dad = if a.dad_category { DAD_CATEGORY } else { DAD_NOTECARD };
    out.put(
        cid,
        "in",
        &format!(
            "pcode={} t={} o={} base={:08x} own={:08x} evr={:08x} dad={}",
            obj.pcode,
            it.ty.0,
            if a.item_owner_agent { "agent" } else { "other" },
            it.perm.base,
            it.perm.owner_mask,
            it.perm.everyone,
            dad
        ),
    );
    let r = give::will_object_accept(&f, if a.null_obj { None } else { Some(&obj) }, if a.null_item { None } else { Some(&it) }, dad);
    out.put(cid, "result", &format!("{} {}", r as i32, NAMES[r as usize]));
}

fn for_each_accept_case(mut fun: impl FnMut(&AcceptCase)) {
    let b = [false, true];
    for m in b {
        for ad in b {
            for o in b {
                for v in b {
                    for at in b {
                        for io in b {
                            for itr in b {
                                for u in b {
                                    for t in 0..5 {
                                        for w in b {
                                            for fi in b {
                                                for p in b {
                                                    for dc in b {
                                                        for rlv in 0..5 {
                                                            fun(&AcceptCase {
                                                                modify: m,
                                                                add: ad,
                                                                you_owner: o,
                                                                volume: v,
                                                                attachment: at,
                                                                item_owner_agent: io,
                                                                item_transfer: itr,
                                                                unrestricted: u,
                                                                ty: t,
                                                                worn: w,
                                                                finished: fi,
                                                                parent_is_object: p,
                                                                dad_category: dc,
                                                                rlv,
                                                                ..AcceptCase::default()
                                                            });
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn will_accept_legend(out: &mut Out) {
    let lines = [
        format!("object {} root {} (sitting_on=root: the avatar sits on that root; object: on the object)", us(&uid(0x41, 1)), us(&uid(0x41, 2))),
        format!(
            "item {} name=\"drop\" creator={} group=null flags=0; parent {} or the object; link (type 24) to {} (a notecard owned by the agent, PERM_ALL)",
            us(&uid(0x42, 1)),
            us(&creator()),
            us(&root()),
            us(&uid(0x42, 2))
        ),
        "perm: next=7fffffff group=0; worn=1: attached for objects, worn by gAgentWearables otherwise".to_owned(),
        format!("owners: agent={} other={}; pcode 9 = LL_PCODE_VOLUME, 47 = not a volume", us(&agent()), us(&other2())),
        "case id: obj_m<perm modify>_a<allow inventory drop>_o<you own it>_v<volume>_att<attachment>.item_own<owned by agent>_t<transfer in owner mask>_u<base unrestricted>_<kind>_w<worn>_f<finished>_p<parent is the object>.dad_<item|category>.rlv<0 off, 1 on, 2 locked attachment, 3 @unsit sitting on the root, 4 @sittp sitting elsewhere>".to_owned(),
        "in: pcode, t=asset type, o=item owner, base/own/evr = the item's masks after initMasks, dad=the EDragAndDropType passed".to_owned(),
    ];
    for (n, l) in lines.iter().enumerate() {
        out.put("legend", &format!("input.{n:02}"), l);
    }
}

#[test]
fn will_accept() {
    let mut out = Out::default();
    will_accept_legend(&mut out);
    let mut n = 0;
    for_each_accept_case(|a| {
        if a.rlv == 0 && !a.dad_category && a.finished && !a.parent_is_object {
            run_accept(&mut out, a, &accept_id(a));
            n += 1;
        }
    });
    let a = AcceptCase {
        modify: true,
        add: true,
        you_owner: true,
        volume: true,
        item_owner_agent: true,
        item_transfer: true,
        unrestricted: true,
        finished: true,
        ty: 3,
        ..AcceptCase::default()
    };
    run_accept(&mut out, &AcceptCase { null_obj: true, ..a }, "null_object");
    run_accept(&mut out, &AcceptCase { null_item: true, ..a }, "null_item");
    assert_lines("give/will_accept", &compared("give", "will_accept.txt"), &out.lines);
    println!("give/will_accept: {n} matrix cases + 2 null cases");
}

#[test]
fn will_accept_sweep() {
    let mut col = Collect::new(401, 512, false);
    let mut out = Out::default();
    will_accept_legend(&mut out);
    col.lines_from(std::mem::take(&mut out.lines));
    for_each_accept_case(|a| {
        run_accept(&mut out, a, &accept_id(a));
        col.lines_from(std::mem::take(&mut out.lines));
    });
    check_sweep("give", "will_accept_sweep", col, None);
}

// ======================================================== world to agent

#[test]
fn world_to_agent() {
    let mut out = Out::default();
    let legend = [
        format!(
            "object {} category {}; the task inventory root ({}, AT_CATEGORY \"Contents\") comes first and is filtered out",
            us(&uid(0x51, 1)),
            us(&uid(0x54, 1)),
            us(&uid(0x51, 1))
        ),
        format!(
            "contents position i (1-based): id 00000052-0000-4000-8000-<i as 12 hex digits>, a notecard named \"item <i-1>\" with asset 00000053-...-<i>, creator {}, owner (and last owner) the agent if you_owner=1 else {}, group null",
            us(&creator()),
            us(&other2())
        ),
        format!(
            "c: base={:08x} owner_mask={:08x} everyone={:08x}; n: base={:08x} owner_mask={:08x} everyone={:08x}; t: base={:08x} owner_mask={:08x} everyone={:08x}; x: an LLInventoryObject of type AT_OBJECT named \"inv_object\" (not an item)",
            PERM_ALL,
            PERM_ALL,
            PERM_COPY,
            PERM_ALL,
            perm_mask(false, true, true),
            PERM_NONE,
            PERM_ALL,
            perm_mask(true, true, false),
            PERM_COPY
        ),
        "passes: the last 12 hex digits of the ids the filter's check() accepts; filter_types 1 = FILTERTYPE_OBJECT, 8 = FILTERTYPE_DATE".to_owned(),
    ];
    for (n, l) in legend.iter().enumerate() {
        out.put("legend", &format!("input.{n:02}"), l);
    }
    let kinds = ['c', 'n', 't', 'x'];
    let mut seqs: Vec<String> = vec![String::new()];
    for len in 1..=3 {
        let next: Vec<String> = seqs.iter().filter(|p| p.len() == len - 1).flat_map(|p| kinds.iter().map(move |k| format!("{p}{k}"))).collect();
        seqs.extend(next);
    }
    for seq in &seqs {
        for owned in [false, true] {
            for filter in 0..4 {
                for drop in [false, true] {
                    for exists in [false, true] {
                        if !exists && (filter != 0 || drop || owned || !seq.is_empty()) {
                            continue;
                        }
                        let mut f = setup();
                        let obj_id = uid(0x51, 1);
                        let mut inventory = vec![TaskObject { id: obj_id, ty: AssetType::CATEGORY, perm: None }];
                        let mut flt = PassFilter { types: match filter { 1 => 1, 2 | 3 => 8, _ => 0 }, passes: BTreeSet::new() };
                        let task_owner = if owned { agent() } else { other2() };
                        for (i, k) in seq.chars().enumerate() {
                            let id = uid(0x52, i as u64 + 1);
                            if k == 'x' {
                                inventory.push(TaskObject { id, ty: AssetType::OBJECT, perm: None });
                                continue;
                            }
                            let mask = match k {
                                'n' => perm_mask(false, true, true),
                                't' => perm_mask(true, true, false),
                                _ => PERM_ALL,
                            };
                            let everyone = if k == 'n' { PERM_NONE } else { PERM_COPY };
                            let mut perm = make_perms(&task_owner, mask, everyone, PERM_ALL, &NULL_ID, false);
                            perm.init_masks_for_type(vkstorm_inventory::types::InventoryType::NOTECARD);
                            inventory.push(TaskObject { id, ty: AssetType::NOTECARD, perm: Some(perm) });
                            if filter == 2 || (filter == 3 && k == 'c') {
                                flt.passes.insert(id);
                            }
                        }
                        if exists {
                            f.objects.insert(
                                obj_id,
                                ObjectRec {
                                    obj: ViewerObject {
                                        id: obj_id,
                                        perm_modify: false,
                                        flag_allow_inventory_add: false,
                                        perm_you_owner: owned,
                                        pcode: LL_PCODE_VOLUME,
                                        is_attachment: false,
                                        root: obj_id,
                                    },
                                    inventory,
                                },
                            );
                        }
                        let cid = format!(
                            "seq_{}.owned{}.filter{}.drop{}.exists{}",
                            if seq.is_empty() { "empty" } else { seq },
                            owned as i32,
                            filter,
                            drop as i32,
                            exists as i32
                        );
                        let passes: Vec<String> = flt.passes.iter().map(|id| us(id)[24..].to_owned()).collect();
                        out.put(
                            &cid,
                            "input",
                            &format!(
                                "object found={} you_owner={} drop={} filter_types={} passes=[{}] contents={}",
                                exists as i32,
                                owned as i32,
                                drop as i32,
                                flt.types,
                                passes.join(","),
                                if seq.is_empty() { "-" } else { seq }
                            ),
                        );
                        let r = if filter != 0 {
                            give::world_to_agent(&mut f, &obj_id, &uid(0x54, 1), drop, Some(&mut flt))
                        } else {
                            give::world_to_agent(&mut f, &obj_id, &uid(0x54, 1), drop, None)
                        };
                        f.add(format!("result {}", r as i32));
                        out.events(&mut f, &cid);
                    }
                }
            }
        }
    }
    assert_lines("give/world_to_agent", &compared("give", "world_to_agent.txt"), &out.lines);
}

// ================================================================ share

#[test]
fn share() {
    let (a, b, c, d) = (uid(0x61, 1), uid(0x61, 2), uid(0x61, 3), uid(0x61, 4));
    let cases: Vec<(&str, Vec<(Uuid, char)>, usize, i32)> = vec![
        ("one_item", vec![(a, 'c')], 1, 0),
        ("cancel", vec![(a, 'c')], 1, 1),
        ("two_items", vec![(a, 'c'), (b, 'c')], 1, 0),
        ("folder_first_then_item", vec![(a, 'f'), (b, 'c')], 1, 0),
        ("item_then_folder_then_item", vec![(a, 'c'), (b, 'f'), (c, 'c')], 1, 0),
        ("two_folders", vec![(a, 'f'), (b, 'f')], 1, 0),
        ("nocopy_items", vec![(a, 'n'), (b, 'c'), (c, 'n')], 1, 0),
        ("nocopy_two_avatars", vec![(a, 'n'), (b, 'c')], 2, 0),
        ("copy_two_avatars", vec![(a, 'c'), (b, 'c')], 2, 0),
        ("folder_two_avatars", vec![(a, 'f'), (d, 'c')], 2, 0),
        ("nocopy_folder", vec![(a, 'g')], 1, 0),
        ("no_avatars", vec![(a, 'c')], 0, 0),
        ("empty_selection", vec![], 1, 0),
    ];
    let mut out = Out::default();
    for (id, sel, avatars_n, option) in cases {
        let mut f = setup();
        let mut selected = BTreeSet::new();
        for (sid, k) in &sel {
            selected.insert(*sid);
            if *k == 'f' || *k == 'g' {
                f.add_cat(*sid, root(), b"Shared Folder", -1);
                f.add_item(uid(0x62, sid[15] as u64), *sid, 7, &perms_simple(&agent(), perm_mask(*k == 'f', true, true)), b"inside", None);
            } else {
                f.add_item(*sid, root(), 7, &perms_simple(&agent(), perm_mask(*k == 'c', true, true)), b"Shared Item", None);
            }
        }
        let avatars: Vec<Uuid> = (0..avatars_n).map(|i| if i == 0 { other() } else { other2() }).collect();
        f.ev.clear();
        let mut notification = Value::Undefined;
        insert(&mut notification, "name", Value::string("ShareItemsConfirmation"));
        let btn = |name: &str, index: i32| {
            let mut e = Value::Undefined;
            insert(&mut e, "type", Value::string("button"));
            insert(&mut e, "name", Value::string(name));
            insert(&mut e, "index", Value::Integer(index));
            e
        };
        insert(&mut notification, "form", Value::Array(vec![btn("OK", 0), btn("Cancel", 1)]));
        let mut response = Value::Undefined;
        insert(&mut response, if option == 0 { "OK" } else { "Cancel" }, Value::Boolean(true));
        put_world(&mut out, &f, id);
        let av: String = avatars.iter().map(|a| format!(" {}", us(a))).collect();
        out.put(id, "input.90", &format!("share option={option} avatars=[{av} ] selected={}", id_set(&selected)));
        f.add(format!("share option={option} selected={} avatars={avatars_n}", selected.len()));
        let sel: Vec<Uuid> = selected.iter().copied().collect();
        give::share(&mut f, &avatars, &notification, &response, &sel);
        f.run_idle();
        out.events(&mut f, id);
    }
    assert_lines("give/share", &compared("give", "share.txt"), &out.lines);
}

// ======================================================== drag and drop

#[test]
fn drag_give() {
    let mut out = Out::default();
    let types: [(&str, i32); 10] = [
        ("texture", DAD_TEXTURE),
        ("object", DAD_OBJECT),
        ("notecard", DAD_NOTECARD),
        ("category", DAD_CATEGORY),
        ("callingcard", DAD_CALLINGCARD),
        ("settings", DAD_SETTINGS),
        ("material", DAD_MATERIAL),
        ("link", DAD_LINK),
        ("widget", DAD_WIDGET),
        ("person", DAD_PERSON),
    ];
    let mut k = 0;
    for (_, ty) in types {
        for state in 0..5 {
            for blocked in [false, true] {
                let mut f = setup();
                let id = uid(0x81, 1);
                let (cargo, rec) = if state == 3 {
                    let c = f.add_cat(id, root(), b"Dragged Folder", -1);
                    let line = f.cat_rec_line(&c);
                    (Cargo::Category(c), line)
                } else {
                    let it = Fake::loose_item(
                        id,
                        root(),
                        if state == 1 { 6 } else { 7 },
                        &perms_simple(&agent(), perm_mask(true, true, state != 4)),
                        b"Dragged",
                        uid(0x82, 1),
                        7,
                    );
                    if state != 2 {
                        f.insert_item(it.clone());
                    }
                    if state == 1 {
                        f.attachments.insert(id);
                    }
                    let line = f.item_rec_line(&it);
                    (Cargo::Item(it.rec), line)
                };
                f.rlv_enabled = blocked;
                if blocked {
                    f.rlv_share_blocked.insert(other());
                }
                let cid = format!("handle_{k:02}_type{ty}_state{state}_blocked{}", blocked as i32);
                k += 1;
                f.ev.clear();
                out.put(
                    &cid,
                    "input",
                    &format!(
                        "{rec} | in_model={} attached={} rlv={} share_blocked_for={} dest={}",
                        (state != 2) as i32,
                        (state == 1) as i32,
                        blocked as i32,
                        if blocked { us(&other()) } else { "-".into() },
                        us(&other())
                    ),
                );
                let (handled, accept) = give::give_drag_and_drop(&mut f, &other(), &NULL_ID, false, ty, &cargo, &Value::Undefined);
                f.add(format!("result handled={} accept={} {}", handled as i32, accept as i32, NAMES[accept as usize]));
                out.events(&mut f, &cid);
            }
        }
    }
    let drops: [(&str, bool, bool, i32, bool); 5] = [
        ("drop_no_session_name_cached_ok", false, true, 0, false),
        ("drop_no_session_name_cached_cancel", false, true, 1, false),
        ("drop_no_session_name_lookup_ok", false, false, 0, false),
        ("drop_in_session_ok", true, true, 0, false),
        ("drop_folder_ok", false, true, 0, true),
    ];
    for (id, session, name_cached, answer, folder) in drops {
        let mut f = setup();
        let cid = uid(0x81, 1);
        let cargo = if folder {
            let c = f.add_cat(cid, root(), b"Dragged Folder", -1);
            f.add_item(uid(0x83, 1), cid, 7, &perms_simple(&agent(), PERM_ALL), b"inside", None);
            Cargo::Category(c)
        } else {
            Cargo::Item(f.add_item(cid, root(), 7, &perms_simple(&agent(), PERM_ALL), b"Dragged", None))
        };
        if !name_cached {
            f.avatar_names.remove(&other());
        }
        let s = give::compute_session_id(&agent(), &other());
        if session {
            f.im_sessions.insert(s);
        }
        f.ev.clear();
        put_world(&mut out, &f, id);
        let (handled, accept) = give::give_drag_and_drop(
            &mut f,
            &other(),
            &if session { s } else { NULL_ID },
            true,
            if folder { DAD_CATEGORY } else { DAD_NOTECARD },
            &cargo,
            &Value::string("avatarpicker"),
        );
        f.add(format!("result handled={} accept={} {}", handled as i32, accept as i32, NAMES[accept as usize]));
        f.run_idle();
        if !f.pending.is_empty() {
            f.respond_pending(answer);
        }
        f.run_idle();
        out.events(&mut f, id);
    }
    let kinds = ["dad3dGiveInventoryObject", "dad3dGiveInventory", "dad3dGiveInventoryCategory"];
    for kind in 0..3 {
        for source in 0..2 {
            for state in 0..4 {
                for blocked in [false, true] {
                    for has_obj in [false, true] {
                        for drop in [false, true] {
                            let mut f = setup();
                            let it_id = uid(0x84, 1);
                            f.add_item(it_id, root(), if kind == 0 { 6 } else { 7 }, &perms_simple(&agent(), perm_mask(true, true, state != 2)), b"Dropped", None);
                            f.items.get_mut(&it_id).unwrap().rec.finished = state != 1;
                            if state == 3 {
                                f.attachments.insert(it_id);
                            }
                            let cat = f.add_cat(uid(0x85, 1), root(), b"Dropped Folder", -1);
                            f.add_item(uid(0x86, 1), cat.id, 7, &perms_simple(&agent(), PERM_ALL), b"inside", None);
                            let tool = DragState {
                                source: if source == 1 { DragSource::World } else { DragSource::Agent },
                                item: if kind == 2 { None } else { f.item(&it_id) },
                                cat: if kind == 2 { Some(cat.clone()) } else { None },
                            };
                            f.rlv_enabled = blocked;
                            if blocked {
                                f.rlv_share_blocked.insert(other());
                            }
                            let cid = format!(
                                "{}.source_{}.state{}.blocked{}.obj{}.drop{}",
                                kinds[kind],
                                if source == 1 { "world" } else { "agent" },
                                state,
                                blocked as i32,
                                has_obj as i32,
                                drop as i32
                            );
                            f.ev.clear();
                            let rec = if kind == 2 { f.cat_rec_line(&cat) } else { f.item_rec_line(&f.items[&it_id]) };
                            out.put(
                                &cid,
                                "input",
                                &format!(
                                    "{rec} | source={} attached={} rlv={} share_blocked_for={} avatar_object={}",
                                    source,
                                    (state == 3) as i32,
                                    blocked as i32,
                                    if blocked { us(&other()) } else { "-".into() },
                                    if has_obj { us(&other()) } else { "null".into() }
                                ),
                            );
                            let o = other();
                            let r = give::dad3d_give(&mut f, kind as i32, &tool, if has_obj { Some(&o) } else { None }, drop);
                            f.add(format!("result accept={} {}", r as i32, NAMES[r as usize]));
                            f.run_idle();
                            if !f.pending.is_empty() {
                                f.respond_pending(0);
                            }
                            out.events(&mut f, &cid);
                        }
                    }
                }
            }
        }
    }
    assert_lines("give/drag_give", &compared("give", "drag_give.txt"), &out.lines);
}

// ================================================================ sweeps

#[test]
fn pack_sweep() {
    let mut rng = Rng::new(0x5234_4250_414b_0001);
    let mut col = Collect::new(100, 512, false);
    for i in 0..4000 {
        let mut f = setup();
        let len = if rng.below(4) == 0 { rng.below(1400) } else { rng.below(80) };
        let mut name = Vec::with_capacity(len as usize);
        for _ in 0..len {
            let c = if rng.below(5) == 0 { rng.below(256) as u8 } else { b'a' + rng.below(26) as u8 };
            name.push(c);
        }
        let bsize = if rng.below(3) == 0 { rng.below(1500) } else { 17 * (1 + rng.below(50)) };
        let bucket: Vec<u8> = (0..bsize).map(|_| rng.below(256) as u8).collect();
        let null_bucket = rng.below(10) == 0;
        let dialog = rng.below(43) as i32;
        let from_group = rng.coin();
        f.act(Action::PackInstantMessage(PackArgs {
            from_id: agent(),
            from_group,
            session_id: session(),
            to_id: other(),
            name: b"Oracle Tester".to_vec(),
            message: name,
            offline: IM_ONLINE,
            dialog,
            id: txn(),
            parent_estate_id: 0,
            region_id: NULL_ID,
            position: [1.0, 2.0, 3.0],
            timestamp: NO_TIMESTAMP,
            // an empty std::vector's data() is null: packed as no bucket
            bucket: if null_bucket || bucket.is_empty() { None } else { Some(bucket) },
        }));
        f.act(Action::SendPackedToRegion);
        let mut out = Out::default();
        out.events(&mut f, &format!("pack_{i:04}"));
        col.lines_from(out.lines);
    }
    check_sweep("give", "pack_sweep", col, None);
}

#[test]
fn folder_sweep() {
    let mut rng = Rng::new(0x5234_4246_4f4c_0003);
    let mut col = Collect::new(60, 512, false);
    fn gen_tree(rng: &mut Rng, ids: &mut Ids, depth: i32) -> Node {
        let cat_name = rng.below(100);
        let mut n = ids.cat(format!("f{cat_name}").as_bytes(), vec![], false);
        n.incomplete = rng.below(25) == 0;
        let kids = rng.below(if depth >= 3 { 4 } else { 9 });
        for _ in 0..kids {
            let sub = depth < 3 && rng.below(4) == 0;
            if sub {
                let child = gen_tree(rng, ids, depth + 1);
                n.children.push(child);
            } else {
                const TYPES: [i32; 9] = [0, 5, 6, 7, 10, 13, 20, 21, 24];
                let worn = if rng.below(8) == 0 { 1 + rng.below(2) as i32 } else { 0 };
                let transfer = rng.below(6) != 0;
                let copy = rng.below(4) != 0;
                let ty = TYPES[rng.below(9) as usize];
                let item_name = rng.below(1000);
                let it = ids.item(format!("i{item_name}").as_bytes(), ty, copy, transfer, worn);
                n.children.push(it);
            }
        }
        n
    }
    for i in 0..1500 {
        let mut f = setup();
        let mut ids = Ids(0);
        let root_node = gen_tree(&mut rng, &mut ids, 0);
        build(&mut f, &root_node, root());
        let limit = if rng.below(5) == 0 { 100 } else { 42 };
        f.max_transfer = limit;
        f.ev.clear();
        let cid = format!("folder_{i:04}");
        let mut out = Out::default();
        put_world(&mut out, &f, &cid);
        let cat = f.category(&root_node.id);
        let r = give::give_category(&mut f, &other(), cat.as_ref(), &NULL_ID, b"");
        f.add(format!("result {}", r as i32));
        if !f.pending.is_empty() {
            let option = if rng.below(4) == 0 { 1 } else { 0 };
            f.respond_pending(option);
        }
        f.run_idle();
        out.events(&mut f, &cid);
        col.lines_from(out.lines);
    }
    check_sweep("give", "folder_sweep", col, None);
}

#[test]
fn predicate_sweep() {
    let mut rng = Rng::new(0x5234_4250_5245_0004);
    let mut col = Collect::new(100, 512, false);
    const TYPES: [i32; 11] = [0, 2, 5, 6, 7, 13, 20, 21, 24, 25, 56];
    for i in 0..20000 {
        let mut f = setup();
        f.groups.insert(group());
        f.avatar_valid = rng.below(10) != 0;
        let owners = [agent(), other2(), NULL_ID];
        let o = rng.below(3) as usize;
        let with_group = rng.coin();
        let mut perm = Permissions::default();
        perm.init(&creator(), &owners[o], &owners[o], &if with_group { group() } else { NULL_ID });
        let next = rng.next() as u32;
        let grp = rng.next() as u32;
        let everyone = rng.next() as u32;
        let owner = rng.next() as u32;
        let base = rng.next() as u32;
        perm.init_masks(base, owner, everyone, grp, next);
        if o == 2 {
            perm.set_owner_and_group(&NULL_ID, &NULL_ID, &group(), true);
        }
        let ty = TYPES[rng.below(11) as usize];
        f.add_item(uid(0x71, 2), root(), 7, &perms_simple(&agent(), PERM_ALL), b"target", None);
        f.add_item(uid(0x71, 1), root(), ty, &perm, b"item", if ty == 24 { Some(uid(0x71, 2)) } else { Some(NULL_ID) });
        let worn = rng.below(4);
        let wid = if ty == 24 { uid(0x71, 2) } else { uid(0x71, 1) };
        match worn {
            1 => {
                f.attachments.insert(wid);
            }
            2 => {
                f.wearables.insert(wid);
            }
            3 => {
                f.gestures.insert(wid);
            }
            _ => {}
        }
        let it = f.item(&uid(0x71, 1)).unwrap();
        // the line shows the permissions the case built (before the item
        // constructor's initMasks(inv_type))
        let p = &perm;
        let (gv, gvc) = functor_out(&f, "giveable", Some(&it), None);
        let (dt, dtc) = functor_out(&f, "droppable_transfer", Some(&it), None);
        let line = format!(
            "pred_{i:05}\tresult\tperm base={:08x} owner={:08x} group={:08x} everyone={:08x} next={:08x} owner_kind={o} type={ty} worn={worn} avatar={} -> give={} group_give={} giveable={}/{} uncopyable={} droppable_transfer={}/{} drop_copyable={}",
            p.base,
            p.owner_mask,
            p.group_mask,
            p.everyone,
            p.next,
            f.avatar_valid as i32,
            give::is_give_acceptable(&f, Some(&it)) as i32,
            give::is_group_give_acceptable(&f, Some(&it)) as i32,
            gv as i32,
            gvc,
            functor_out(&f, "uncopyable", Some(&it), None).0 as i32,
            dt as i32,
            dtc,
            functor_out(&f, "drop_copyable", Some(&it), None).0 as i32
        );
        col.line(&line);
    }
    check_sweep("give", "predicate_sweep", col, None);
}
