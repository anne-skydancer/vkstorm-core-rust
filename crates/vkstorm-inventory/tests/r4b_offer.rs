//! R4b offers: regenerates `fixtures/offer/{responses, incoming,
//! incoming_sweep.*, offerinfo_llsd, forced_response_index, throttle,
//! offline, policy_named}` from the crate's offer decisions
//! (`vkstorm_inventory::exchange::offer`) run against the harness's fakes
//! (`r4b_common`), as `llinventory_offer_oracle.cpp` records them from the
//! frozen C++. Notes (`note.*`) are neither compared nor produced.

mod r4b_common;

use r4b_common::incoming::*;
use r4b_common::*;
use vkstorm_inventory::exchange::offer;
use vkstorm_inventory::exchange::throttle::check_offer_throttle;
use vkstorm_inventory::exchange::{NetHost, OfferInfo};
use vkstorm_inventory::llsd::coerce::insert;
use vkstorm_inventory::perms::{Uuid, NULL_ID};
use vkstorm_inventory::types::AssetType;
use vkstorm_llsd::Value;

fn make_offer(im: i32, t: Uuid) -> OfferInfo {
    OfferInfo {
        im,
        from_id: other(),
        from_group: false,
        from_object: im == 9,
        transaction_id: t,
        folder_id: folder_for(7),
        object_id: if im == 4 { offered() } else { NULL_ID },
        ty: AssetType::NOTECARD,
        from_name: b"other.sender".to_vec(),
        desc: b"Gift".to_vec(),
        host: NetHost::parse(OFFER_HOST),
        persist: false,
    }
}

#[test]
fn responses() {
    let mut out = Out::default();
    let dest = uid(0xde, 1);
    type Op = (&'static str, fn(&mut Fake, &mut OfferInfo, &Uuid));
    let ops: [Op; 9] = [
        ("accept_dest", |f, o, d| offer::send_receive_response(f, o, true, d)),
        ("accept_null_dest", |f, o, _| offer::send_receive_response(f, o, true, &NULL_ID)),
        ("decline", |f, o, d| offer::send_receive_response(f, o, false, d)),
        ("accept_then_decline", |f, o, d| {
            offer::send_receive_response(f, o, true, d);
            offer::send_receive_response(f, o, false, d);
        }),
        ("auto_receive", |f, o, _| offer::send_auto_receive_response(f, o)),
        ("legacy_decline", |f, o, _| offer::send_decline_response(f, o)),
        ("legacy_decline_twice", |f, o, _| {
            offer::send_decline_response(f, o);
            offer::send_decline_response(f, o);
        }),
        ("accept_avatar_invalid", |f, o, d| {
            f.avatar_valid = false;
            offer::send_receive_response(f, o, true, d)
        }),
        ("accept_rlv_names_hidden_nearby", |f, o, d| {
            f.rlv_enabled = true;
            f.rlv_can_show_names = false;
            f.rlv_nearby.insert(other());
            offer::send_receive_response(f, o, true, d)
        }),
    ];
    for d in [4, 9, 32, 37] {
        for txn_null in [false, true] {
            let t = if txn_null { NULL_ID } else { txn() };
            let base = format!("d{d}.{}", if txn_null { "txn_null" } else { "txn" });
            for (id, op) in &ops {
                let mut f = setup();
                f.snapshots = true;
                let h = f.new_offer(make_offer(d, t));
                f.cur = Some(h);
                f.add(format!("offer {}", offer_fields(&f.offer(h))));
                let mut o = f.offer(h);
                op(&mut f, &mut o, &dest);
                let after = f.offer(h);
                f.add(format!("after {}", offer_fields(&after)));
                out.events(&mut f, &format!("{base}.{id}"));
            }
        }
    }
    assert_lines("offer/responses", &compared("offer", "responses.txt"), &out.lines);
}

fn incoming_cases() -> Vec<Incoming> {
    let mut v = Vec::new();
    for sz in [0usize, 1, 16, 18, 34] {
        v.push(Incoming::new(&format!("agent_size_{sz}"), 4, vec![7; sz]));
    }
    for t in [0i32, 6, 7, 8, 13, 24, 25, 56, 59, 60, 127, -1, -2, -128] {
        v.push(Incoming::new(&format!("agent_type_{t}"), 4, agent_bucket(t as i8, &offered())));
    }
    {
        let mut c = Incoming::new("agent_null_object", 4, agent_bucket(7, &NULL_ID));
        v.push(c.clone());
        c.id = "agent_session_null_aux".into();
        c.bucket = agent_bucket(7, &offered());
        c.session = NULL_ID;
        c.aux = uid(0xd4, 2);
        v.push(c.clone());
        c.id = "agent_session_and_aux_null".into();
        c.aux = NULL_ID;
        v.push(c.clone());
        c.id = "agent_session_and_aux".into();
        c.session = txn();
        c.aux = uid(0xd4, 2);
        v.push(c.clone());
        c.id = "agent_name_resident".into();
        c.aux = NULL_ID;
        c.name = b"Sender Resident".to_vec();
        v.push(c.clone());
        c.id = "agent_name_blank".into();
        c.name = b"   ".to_vec();
        v.push(c.clone());
        c.id = "agent_name_legacy".into();
        c.name = b"Sender Lastname".to_vec();
        v.push(c.clone());
        c.id = "agent_name_resident_untrimmed".into();
        c.name = b"Sender Resident".to_vec();
        c.trim = false;
        v.push(c.clone());
        c.trim = true;
        c.id = "agent_from_group".into();
        c.name = b"Sender Resident".to_vec();
        c.from_group = true;
        v.push(c.clone());
        c.id = "agent_from_self".into();
        c.from_group = false;
        c.from = agent();
        v.push(c.clone());
    }
    let gadget = b"'Gadget'  ( http://slurl.com/secondlife/Region/1/2/3 )".to_vec();
    for t in [0u8, 6, 7, 8, 24, 59, 60, 127, 128, 254, 255] {
        let mut c = Incoming::new(&format!("task_type_{t}"), 9, vec![t]);
        c.message = gadget.clone();
        c.name = b"Gadget".to_vec();
        v.push(c);
    }
    let legacy: [(&str, &[u8], bool); 14] = [
        ("7_pipe_nul", b"7|abc", true),
        ("7_pipe_no_nul", b"7|abc", false),
        ("8_pipe_nul", b"8|folder|x", true),
        ("7_no_pipe_nul", b"7", true),
        ("77_no_pipe_no_nul", b"77", false),
        ("pipe_only", b"|x", true),
        ("empty_type_no_nul", b"|", false),
        ("space_7", b" 7|x", true),
        ("neg_2", b"-2|x", true),
        ("overflow", b"99999999999|x", true),
        ("alpha", b"x|y", true),
        ("7x", b"7x|y", true),
        ("plus_8", b"+8|y", true),
        ("leading_nul", b"\x007|x", false),
    ];
    for (id, text, nul) in legacy {
        for fill in 0..2 {
            let mut b = text.to_vec();
            if nul {
                b.push(0);
            }
            let mut c = Incoming::new(&format!("task_legacy_{id}_fill{fill}"), 9, b);
            c.fill = fill;
            c.message = gadget.clone();
            c.name = b"Gadget".to_vec();
            v.push(c);
        }
    }
    for sz in [0usize, 16, 17, 18, 34] {
        for fill in 0..2 {
            let b = if sz == 17 { agent_bucket(7, &offered()) } else { vec![b'7'; sz] };
            let mut c = Incoming::new(&format!("task_size_{sz}_fill{fill}"), 9, b);
            c.fill = fill;
            c.name = b"Gadget".to_vec();
            c.message = gadget.clone();
            v.push(c);
        }
    }
    {
        let mut c = Incoming::new("task_declared_1300", 9, vec![b'7'; 1300]);
        c.name = b"Gadget".to_vec();
        v.push(c.clone());
        c.id = "task_session_null_aux".into();
        c.bucket = vec![7];
        c.session = NULL_ID;
        c.aux = uid(0xd4, 2);
        v.push(c.clone());
        c.id = "task_from_group".into();
        c.session = txn();
        c.aux = NULL_ID;
        c.from_group = true;
        c.from = group();
        v.push(c.clone());
        c.id = "task_from_self".into();
        c.from_group = false;
        c.from = agent();
        v.push(c.clone());
        c.id = "task_name_resident_kept".into();
        c.from = other();
        c.name = b"Gadget Resident".to_vec();
        v.push(c.clone());
    }
    for d in [32, 37] {
        let p = format!("d{d}_");
        let mut c = Incoming::new(&format!("{p}bin_full"), d, notice_bucket(1, 7, &group(), b"Notice Item", true));
        c.message = b"Subject|Body text".to_vec();
        c.name = b"Officer Resident".to_vec();
        v.push(c.clone());
        let mut add = |c: &mut Incoming, id: &str| {
            c.id = format!("{p}{id}");
            v.push(c.clone());
        };
        c.bucket = notice_bucket(0, 7, &group(), b"Notice Item", true);
        add(&mut c, "bin_no_inventory");
        c.bucket = notice_bucket(1, 7, &group(), b"", true);
        add(&mut c, "bin_19_empty_name");
        c.bucket = notice_bucket(1, 7, &group(), b"", false);
        add(&mut c, "bin_18_short");
        c.bucket = notice_bucket(1, 7, &group(), b"Item", false);
        add(&mut c, "bin_not_terminated");
        c.bucket = notice_bucket(1, 8, &group(), b"Kit", true);
        add(&mut c, "bin_type_category");
        c.bucket = notice_bucket(1, 255, &group(), b"Odd", true);
        add(&mut c, "bin_type_255");
        c.bucket = notice_bucket(1, 7, &group(), b"Ab\0cd", true);
        add(&mut c, "bin_embedded_nul");
        c.bucket = notice_bucket(1, 7, &other(), b"Item", true);
        add(&mut c, "bin_group_is_sender");
        c.name = b"Known Resident".to_vec();
        add(&mut c, "bin_group_is_sender_known_name");
        c.bucket = notice_bucket(1, 7, &group(), b"Item", true);
        c.name = us(&other2()).into_bytes();
        add(&mut c, "bin_uuid_name");
        c.name = b"Officer Resident".to_vec();
        add(&mut c, "bin_sender_muted");
        c.session = NULL_ID;
        add(&mut c, "bin_session_null");
        c.session = txn();
        c.message = b"Subject only".to_vec();
        add(&mut c, "message_no_pipe");
        c.message = b"Subject|Body text".to_vec();
        c.aux = group();
        c.bucket = b"7|7|Notice Item".to_vec();
        add(&mut c, "str_full");
        c.bucket = b"5|4|Shirt".to_vec();
        add(&mut c, "str_wearable");
        c.bucket = b"7|7|Item\0".to_vec();
        add(&mut c, "str_nul_terminated");
        c.bucket = b"||".to_vec();
        add(&mut c, "str_empty_fields");
        c.bucket = b"7|7|Item|more".to_vec();
        add(&mut c, "str_extra_fields");
        c.bucket = b"x|7|Item".to_vec();
        add(&mut c, "str_alpha_type");
        c.bucket = vec![0];
        add(&mut c, "str_size_1");
        c.bucket = Vec::new();
        add(&mut c, "str_size_0");
        c.bucket = b"7|7|Item".to_vec();
        c.from_group = false;
        add(&mut c, "str_from_group_false");
    }
    v
}

#[test]
fn incoming() {
    let mut out = Out::default();
    let mut uninit = 0;
    for c in incoming_cases() {
        let mut f = setup();
        f.snapshots = true;
        f.trim_resident = c.trim;
        f.ids_by_legacy_name.insert(b"Known Resident".to_vec(), other2());
        if c.id.contains("sender_muted") {
            f.mute_insert(other(), b"", 1, 0);
        }
        run_incoming(&mut f, &c, true);
        if f.ev.iter().any(|e| e.starts_with("legacy_read ") && e.contains("type=UNINITIALISED")) {
            // the outcome depends on the stack and is kept as notes only
            uninit += 1;
            f.ev.truncate(2);
            f.ev.push("runtime_edge type uninitialised: the outcome depends on the stack and is not compared".into());
        }
        out.events(&mut f, &c.id);
    }
    assert_lines("offer/incoming", &compared("offer", "incoming.txt"), &out.lines);
    println!("offer/incoming: {uninit} uninitialised-type cases compared up to the runtime edge line");
}

#[test]
fn incoming_sweep() {
    let mut rng = Rng::new(0x5234_424f_4646_0002);
    let dialogs = [4, 9, 32, 37];
    let mut col = Collect::new(60, 512, false);
    for i in 0..6000 {
        let mut c = Incoming::new("", dialogs[rng.below(4) as usize], Vec::new());
        let size = if rng.below(4) == 0 {
            rng.below(200) as usize
        } else if c.dialog == 4 {
            17
        } else if c.dialog == 9 {
            1
        } else {
            19 + rng.below(30) as usize
        };
        c.bucket = Vec::with_capacity(size);
        for _ in 0..size {
            let x = if rng.below(3) == 0 {
                rng.below(256) as u8
            } else if rng.below(3) == 0 {
                b'|'
            } else {
                b'0' + rng.below(10) as u8
            };
            c.bucket.push(x);
        }
        if !c.bucket.is_empty() && rng.below(2) == 0 {
            *c.bucket.last_mut().unwrap() = 0;
        }
        c.session = if rng.below(4) == 0 { NULL_ID } else { txn() };
        c.aux = if rng.below(3) == 0 { group() } else { NULL_ID };
        c.from_group = rng.coin();
        c.from = if rng.below(5) == 0 { agent() } else { other() };
        c.message = if rng.coin() { b"Subject|Body".to_vec() } else { b"#RLV/~x".to_vec() };
        c.name = if rng.coin() { b"Sender Resident".to_vec() } else { Vec::new() };
        c.fill = 0;
        if c.dialog == 9 && size != 1 {
            let mut buf = c.bucket.clone();
            buf.push(0);
            if legacy_type_probe(&buf).contains("UNINITIALISED") {
                continue;
            }
        }
        let mut f = setup();
        f.snapshots = true;
        run_incoming(&mut f, &c, false);
        let mut out = Out::default();
        out.events(&mut f, &format!("incoming_{i:04}"));
        col.lines_from(out.lines);
    }
    check_sweep("offer", "incoming_sweep", col, None);
}

#[test]
fn offerinfo_llsd() {
    let mut out = Out::default();
    struct F {
        id: &'static str,
        im: i32,
        from: Uuid,
        group: bool,
        object: bool,
        txn: Uuid,
        folder: Uuid,
        obj: Uuid,
        ty: i32,
        from_name: Vec<u8>,
        desc: Vec<u8>,
        host: &'static [u8],
        persist: bool,
    }
    let f = |id, im, from, group, object, txn, folder, obj, ty, from_name: &[u8], desc: &[u8], host, persist| F {
        id,
        im,
        from,
        group,
        object,
        txn,
        folder,
        obj,
        ty,
        from_name: from_name.to_vec(),
        desc: desc.to_vec(),
        host,
        persist,
    };
    let u7 = uid(0xf0, 7);
    let pipe_name = format!("{}|{{InvOfferGroupNoticeName|NAME=%s}}", us(&other2())).into_bytes();
    let fields = vec![
        f("agent", 4, other(), false, false, txn(), u7, offered(), 7, b"other.sender", b"Gift", OFFER_HOST, true),
        f("task", 9, other(), false, true, txn(), u7, NULL_ID, 7, b"Gadget", b"'Gadget'  ( http://slurl.com/secondlife/R/1/2/3 )", OFFER_HOST, true),
        f("group_notice", 32, other(), true, false, txn(), uid(0xf0, 8), NULL_ID, 8, b"{InvOfferGroupNoticeName|NAME=Officer}", b"Kit", OFFER_HOST, false),
        f("group_requested", 37, group(), true, false, txn(), u7, NULL_ID, 7, b"x", b"y", OFFER_HOST, false),
        f("null_everything", 0, NULL_ID, false, false, NULL_ID, NULL_ID, NULL_ID, -1, b"", b"", b"", false),
        f("type_link", 4, other(), false, false, txn(), u7, offered(), 24, b"a", b"b", OFFER_HOST, false),
        f("type_127", 4, other(), false, false, txn(), u7, offered(), 127, b"a", b"b", OFFER_HOST, false),
        f("type_unknown_255", 9, other(), false, true, txn(), u7, NULL_ID, 255, b"a", b"b", OFFER_HOST, false),
        f("type_minus_2", 4, other(), false, false, txn(), u7, offered(), -2, b"a", b"b", OFFER_HOST, false),
        f("utf8_names", 4, other(), false, false, txn(), u7, offered(), 0, "J\u{e9}r\u{f4}me".as_bytes(), "\u{2713} tick".as_bytes(), OFFER_HOST, false),
        f("uuid_pipe_name", 32, other(), true, false, txn(), u7, NULL_ID, 7, &pipe_name, b"Kit", OFFER_HOST, false),
        f("host_zero", 4, other(), false, false, txn(), u7, offered(), 7, b"a", b"b", b"0.0.0.0:0", false),
        f("host_max_port", 4, other(), false, false, txn(), u7, offered(), 7, b"a", b"b", b"255.255.255.255:65535", false),
    ];
    for x in &fields {
        let o = OfferInfo {
            im: x.im,
            from_id: x.from,
            from_group: x.group,
            from_object: x.object,
            transaction_id: x.txn,
            folder_id: x.folder,
            object_id: x.obj,
            ty: AssetType(x.ty),
            from_name: x.from_name.clone(),
            desc: x.desc.clone(),
            host: if x.host.is_empty() { NetHost::default() } else { NetHost::parse(x.host) },
            persist: x.persist,
        };
        let c = format!("fields_{}", x.id);
        out.put(&c, "in", &offer_fields(&o));
        let sd = o.as_llsd();
        out.put(&c, "asLLSD.bin", &sd_bin_hex(&sd));
        let back = OfferInfo::from_llsd(&sd);
        out.put(&c, "fromLLSD", &offer_fields(&back));
        out.put(&c, "fromLLSD_assign", &offer_fields(&OfferInfo::from_llsd(&sd)));
        let again = back.as_llsd();
        out.put(&c, "round_trip_equal", if sd_bin_hex(&again) == sd_bin_hex(&sd) { "1" } else { "0" });
    }
    // crafted documents: the notation as the C++ string literals hold it
    // (the binary_desc literal ends at its embedded NUL)
    let docs: [(&str, &[u8]); 16] = [
        ("empty_map", b"{}"),
        ("undefined", b"!"),
        ("im_as_string", b"{'im_type':'4','type':'notecard','sender':'10.0.0.9:13005','persist':1}"),
        ("im_as_real", b"{'im_type':r9.7,'type':'object'}"),
        ("type_unknown_name", b"{'im_type':i4,'type':'bogus'}"),
        ("type_empty", b"{'im_type':i4,'type':''}"),
        ("type_upper", b"{'im_type':i4,'type':'NOTECARD'}"),
        ("type_human_readable", b"{'im_type':i4,'type':'notecard'}"),
        ("ids_as_strings", b"{'from_id':'b2000000-0000-4000-8000-000000000001','transaction_id':'not a uuid','folder_id':''}"),
        ("sender_no_port", b"{'sender':'10.0.0.9'}"),
        ("sender_garbage", b"{'sender':'garbage:xx'}"),
        ("sender_name", b"{'sender':'localhost:13000'}"),
        ("bools_as_strings", b"{'from_group':'true','from_object':'false','persist':'1'}"),
        ("bools_as_ints", b"{'from_group':i2,'from_object':i0,'persist':i-1}"),
        ("extra_keys", b"{'responder_type':'offer_info','im_type':i9,'unknown':i1}"),
        ("binary_desc", b"{'description':b(3)\"a"),
    ];
    for (id, notation) in docs {
        let limits = vkstorm_llsd::Limits::for_len(notation.len());
        let (_, sd) = vkstorm_llsd::parse(vkstorm_llsd::Format::Notation, notation, limits);
        let sd = sd.unwrap_or(Value::Undefined);
        let c = format!("doc_{id}");
        out.put(&c, "doc.bin", &sd_bin_hex(&sd));
        let o = OfferInfo::from_llsd(&sd);
        out.put(&c, "fromLLSD", &offer_fields(&o));
        out.put(&c, "asLLSD.bin", &sd_bin_hex(&o.as_llsd()));
    }
    assert_lines("offer/offerinfo_llsd", &compared("offer", "offerinfo_llsd.txt"), &out.lines);
}

#[test]
fn forced_response_index() {
    let mut out = Out::default();
    for name in ["UserGiveItem", "UserGiveItemLegacy", "ObjectGiveItem", "OwnObjectGiveItem"] {
        if let Some(Value::Array(form)) = &notification_form(name) {
            for (i, e) in form.iter().enumerate() {
                let n = String::from_utf8_lossy(&vkstorm_inventory::llsd::coerce::as_string(vkstorm_inventory::llsd::coerce::get(e, "name"))).into_owned();
                let idx = vkstorm_inventory::llsd::coerce::as_integer(vkstorm_inventory::llsd::coerce::get(e, "index"));
                out.put(&format!("form_{name}"), &format!("element{i}"), &format!("name={n} button_index={idx}"));
            }
        }
    }
    let form = match &notification_form("UserGiveItem") {
        Some(Value::Array(a)) => a.clone(),
        _ => unreachable!(),
    };
    let describe = |i: i32| -> String {
        match (i >= 0).then(|| form.get(i as usize)).flatten() {
            Some(e) => format!(
                "{} (button {})",
                String::from_utf8_lossy(&vkstorm_inventory::llsd::coerce::as_string(vkstorm_inventory::llsd::coerce::get(e, "name"))),
                vkstorm_inventory::llsd::coerce::as_integer(vkstorm_inventory::llsd::coerce::get(e, "index"))
            ),
            None => "- (button -1)".into(),
        }
    };
    for r in 0..=6 {
        let mut f = setup();
        f.snapshots = true;
        let mut o = make_offer(4, txn());
        o.persist = true;
        let h = f.new_offer(o);
        f.add(format!("forceResponse({r})"));
        f.force_response(h, r);
        f.run_idle();
        out.events(&mut f, &format!("force_{r}"));
        let element = match r {
            0 => 1,
            1 => 2,
            2 => 3,
            _ => -1,
        };
        out.put(&format!("index_{r}"), "element_used", &format!("{element} -> {}", describe(element)));
        out.put(&format!("index_{r}"), "if_button_index_were_used", &format!("{r} -> {}", describe(r)));
    }
    assert_lines("offer/forced_response_index", &compared("offer", "forced_response_index.txt"), &out.lines);
}

#[test]
fn throttle() {
    let mut out = Out::default();
    let mut burst: Vec<(f64, bool, &[u8])> = vec![(0.0, false, b"giver")];
    for i in 1..=7 {
        burst.push((0.1 * i as f64, false, b"giver"));
    }
    burst.push((0.9, true, b"giver"));
    burst.push((10.5, true, b"giver"));
    burst.push((10.5, false, b"giver"));
    burst.push((10.6, false, b""));
    let anon: Vec<(f64, bool, &[u8])> = burst.iter().map(|&(t, c, _)| (t, c, b"".as_slice())).collect();
    let check_first: Vec<(f64, bool, &[u8])> = vec![(0.0, true, b"x"), (0.0, false, b"x"), (0.0, true, b"x"), (9.99, true, b"x"), (10.0, true, b"x")];
    let traces = [
        ("default_burst", 5, true, true, true, &burst),
        ("anonymous_burst", 5, true, true, true, &anon),
        ("max_2", 2, true, true, true, &burst),
        ("max_0", 0, true, true, true, &burst),
        ("before_started", 5, false, true, true, &burst),
        ("notices_off", 5, true, false, false, &burst),
        ("check_only_first", 5, true, true, true, &check_first),
    ];
    // the statics carry across traces; each starts with an expired timer
    let mut state = vkstorm_inventory::exchange::Throttle::default();
    for (id, max, started, spam_from, spam, steps) in traces {
        let mut f = setup();
        f.settings_u.insert("FSOfferThrottleMaxCount".into(), max);
        f.settings_b.insert("FSNotifyIncomingObjectSpamFrom".into(), spam_from);
        f.settings_b.insert("FSNotifyIncomingObjectSpam".into(), spam);
        f.startup_started = started;
        state.expiry = 0.0;
        f.throttle = state;
        let t0 = 5000.0;
        for &(t, check_only, from) in steps.iter() {
            f.clock = t0 + t;
            let r = check_offer_throttle(&mut f, from, check_only);
            f.add(format!("t={t:.2} check_only={} from=\"{}\" -> {}", check_only as i32, esc(from), r as i32));
        }
        state = f.throttle;
        out.events(&mut f, id);
    }
    assert_lines("offer/throttle", &compared("offer", "throttle.txt"), &out.lines);
}

#[test]
fn offline() {
    let mut out = Out::default();
    let msg = |dialog: i32, bucket: Option<Vec<u8>>| {
        let mut m = Value::Undefined;
        insert(&mut m, "from_agent_id", Value::Uuid(other()));
        insert(&mut m, "from_group", Value::Integer(0));
        insert(&mut m, "to_agent_id", Value::Uuid(agent()));
        insert(&mut m, "dialog", Value::Integer(dialog));
        insert(&mut m, "transaction-id", Value::Uuid(txn()));
        insert(&mut m, "timestamp", Value::Integer(1_700_000_000));
        insert(&mut m, "from_agent_name", Value::string(if dialog == 9 { "Gadget" } else { "Sender Resident" }));
        insert(
            &mut m,
            "message",
            Value::string(if dialog == 9 { "'Gadget'  ( http://slurl.com/secondlife/Region/1/2/3 )" } else { "Gift" }),
        );
        insert(&mut m, "region_id", Value::Uuid(uid(0x9e, 1)));
        insert(&mut m, "position", Value::Array(vec![Value::Real(1.0), Value::Real(2.0), Value::Real(3.0)]));
        if let Some(b) = bucket {
            insert(&mut m, "binary_bucket", Value::Binary(b));
        }
        m
    };
    let set = |mut m: Value, k: &str, v: Value| {
        insert(&mut m, k, v);
        m
    };
    let cases: Vec<(&str, Value, i32, bool)> = vec![
        ("agent_offer_keep", msg(4, Some(agent_bucket(7, &offered()))), 0, false),
        ("agent_offer_discard", msg(4, Some(agent_bucket(7, &offered()))), 1, false),
        ("agent_offer_no_bucket_key", msg(4, None), 0, false),
        (
            "agent_offer_transaction_from_asset_id",
            set(set(msg(4, Some(agent_bucket(7, &offered()))), "transaction-id", Value::Uuid(NULL_ID)), "asset_id", Value::Uuid(uid(0xd4, 2))),
            0,
            false,
        ),
        ("task_offer_keep_replies_to_region_host", msg(9, Some(vec![7])), 0, false),
        ("task_offer_discard_replies_to_region_host", msg(9, Some(vec![7])), 1, false),
        ("task_offer_no_bucket_key", msg(9, None), 0, false),
        ("task_offer_from_group_string_Y", set(msg(9, Some(vec![7])), "from_group", Value::string("Y")), 0, false),
        (
            "task_offer_local_position",
            {
                let mut m = msg(9, Some(vec![7]));
                if let Value::Map(map) = &mut m {
                    map.remove(b"position".as_slice());
                }
                let m = set(m, "local_x", Value::Real(4.5));
                let m = set(m, "local_y", Value::Real(5.5));
                set(m, "local_z", Value::Real(6.5))
            },
            0,
            false,
        ),
        (
            "group_notice_asset_id_is_group",
            set(set(msg(32, Some(b"7|7|Notice Item".to_vec())), "asset_id", Value::Uuid(group())), "message", Value::string("Subject|Body")),
            0,
            true,
        ),
        (
            "group_notice_binary_bucket",
            set(msg(32, Some(notice_bucket(1, 7, &group(), b"Notice Item", true))), "message", Value::string("Subject|Body")),
            0,
            true,
        ),
    ];
    for (id, m, button, forced) in cases {
        let mut f = setup();
        f.snapshots = true;
        let messages = Value::Array(vec![m]);
        out.put(id, "input.00", &format!("messages {}", sd_notation(&messages)));
        out.put(id, "input.01", &format!("agent_region_host {}", f.region_host.ip_and_port()));
        offer::receive_offline(&mut f, &messages);
        f.add("stage main_coro");
        f.run_idle();
        f.add(format!("stage response {} button={button}", if forced { "forced" } else { "interactive" }));
        if let Some(nid) = f.posted.last().copied() {
            let n = f.live.get(&nid).cloned();
            let info = n.as_ref().and_then(|n| n.responder).filter(|h| f.offers.contains_key(h));
            if let (Some(h), true) = (info, forced) {
                f.force_response(h, button);
            } else if let Some(n) = n {
                let mut response = Value::Undefined;
                if let Some(Value::Array(a)) = &n.form {
                    for e in a {
                        if vkstorm_inventory::llsd::coerce::as_integer(vkstorm_inventory::llsd::coerce::get(e, "index")) == button {
                            let k = String::from_utf8_lossy(&vkstorm_inventory::llsd::coerce::as_string(vkstorm_inventory::llsd::coerce::get(e, "name"))).into_owned();
                            insert(&mut response, &k, Value::Boolean(true));
                        }
                    }
                }
                f.respond_notification(&nid, &response);
            }
        } else if vkstorm_inventory::llsd::coerce::has(&f.group_notice_payload, "inventory_offer") {
            let o = OfferInfo::from_llsd(vkstorm_inventory::llsd::coerce::get(&f.group_notice_payload, "inventory_offer"));
            let h = f.new_offer(o);
            f.force_response(h, button);
            if f.offers.contains_key(&h) {
                f.drop_offer_silently(h);
            }
        }
        f.run_idle();
        out.events(&mut f, id);
    }
    assert_lines("offer/offline", &compared("offer", "offline.txt"), &out.lines);
}

#[test]
fn policy_named() {
    let base = |d: i32, button: i32, forced: bool| PolicyCase { dialog: d, button, forced, ty: 1, log_decline: true, ..PolicyCase::default() };
    type Set = fn(&mut PolicyCase);
    let named: Vec<(&str, PolicyCase, Set)> = vec![
        ("agent_offer_accepted_on_arrival_closed", base(4, 7, false), |_| {}),
        ("agent_offer_keep_sends_nothing_more", base(4, 0, false), |_| {}),
        ("agent_offer_discard_moves_to_trash_locally", base(4, 1, false), |_| {}),
        ("agent_offer_mute_button", base(4, 2, false), |_| {}),
        ("agent_offer_mute_button_deferred_names", base(4, 2, false), |p| {
            p.response_table = true;
            p.defer = true
        }),
        ("agent_offer_legacy_keep_sends_accept", base(4, 0, false), |p| p.legacy = true),
        ("agent_offer_legacy_discard_sends_decline_to_trash", base(4, 1, false), |p| p.legacy = true),
        ("agent_offer_muted_by_id_forced_decline", base(4, 0, false), |p| p.mute = 1),
        ("agent_offer_muted_by_name_forced_mute", base(4, 0, false), |p| p.mute = 2),
        ("agent_offer_textchat_exempt_mute_forced_mute", base(4, 0, false), |p| p.mute = 3),
        ("agent_offer_dnd_accepts_and_replies", base(4, 7, false), |p| p.dnd = true),
        ("agent_offer_auto_accept", base(4, 0, false), |p| p.auto_accept = true),
        ("agent_offer_invalid_type_forced_decline", base(4, 0, false), |p| p.ty = 2),
        ("agent_offer_forced_show_does_nothing", base(4, 3, true), |_| {}),
        ("agent_give_to_rlv_keep", base(4, 0, false), |p| {
            p.rlv = true;
            p.give_rlv = 1;
            p.ty = 0
        }),
        ("agent_give_to_rlv_not_auto_accepted", base(4, 0, false), |p| {
            p.rlv = true;
            p.give_rlv = 1;
            p.ty = 0;
            p.auto_accept = true
        }),
        ("agent_give_to_rlv_forbidden_auto_accepted", base(4, 0, false), |p| {
            p.rlv = true;
            p.give_rlv = 1;
            p.ty = 0;
            p.auto_accept = true;
            p.forbid = true
        }),
        ("agent_give_to_rlv_no_shared_root_auto_accepted", base(4, 0, false), |p| {
            p.rlv = true;
            p.give_rlv = 2;
            p.ty = 0;
            p.auto_accept = true
        }),
        ("object_offer_keep", base(9, 0, false), |_| {}),
        ("object_offer_discard_accepts_into_trash", base(9, 1, false), |_| {}),
        ("object_offer_closed_accepts_into_trash", base(9, 7, false), |_| {}),
        ("object_offer_mute_button_declines", base(9, 2, false), |_| {}),
        ("object_offer_mute_button_deferred_names_declines", base(9, 2, false), |p| {
            p.response_table = true;
            p.defer = true
        }),
        ("object_offer_discard_after_late_mute_by_id_declines", base(9, 1, false), |p| {
            p.response_table = true;
            p.late_mute = 1
        }),
        ("object_offer_discard_after_late_mute_by_name_declines", base(9, 1, false), |p| {
            p.response_table = true;
            p.late_mute = 2
        }),
        ("object_offer_keep_after_late_mute_still_accepts", base(9, 0, false), |p| {
            p.response_table = true;
            p.late_mute = 1
        }),
        ("object_offer_group_owned_mute_button_mutes_group", base(9, 2, false), |p| {
            p.response_table = true;
            p.from_group = true
        }),
        ("object_offer_group_owned_mute_button_deferred", base(9, 2, false), |p| {
            p.response_table = true;
            p.from_group = true;
            p.defer = true
        }),
        ("object_offer_own_object_keep", base(9, 0, false), |p| p.self_ = true),
        ("object_offer_own_object_muted_by_name", base(9, 0, false), |p| {
            p.self_ = true;
            p.mute = 2
        }),
        ("object_offer_dnd_no_reply", base(9, 0, false), |p| p.dnd = true),
        ("object_offer_muted_no_reply", base(9, 0, false), |p| p.mute = 1),
        ("object_offer_auto_accept", base(9, 0, false), |p| p.auto_accept = true),
        ("object_offer_invalid_type_forced_decline", base(9, 0, false), |p| p.ty = 2),
        ("object_give_to_rlv_keep_redirects_to_root", base(9, 0, false), |p| {
            p.rlv = true;
            p.give_rlv = 1;
            p.ty = 0
        }),
        ("object_give_to_rlv_forbidden_keep", base(9, 0, false), |p| {
            p.rlv = true;
            p.give_rlv = 1;
            p.ty = 0;
            p.forbid = true
        }),
        ("object_give_to_rlv_discard", base(9, 1, false), |p| {
            p.rlv = true;
            p.give_rlv = 1;
            p.ty = 0
        }),
        ("object_offer_linden_discard_after_late_mute", base(9, 1, false), |p| {
            p.response_table = true;
            p.late_mute = 1;
            p.linden = true
        }),
        ("group_notice_accept", base(32, 0, true), |_| {}),
        ("group_notice_discard_no_dismiss", base(32, 1, true), |_| {}),
        ("group_notice_discard_dismiss_accepts_into_trash", base(32, 1, true), |p| p.dismiss = true),
        ("group_notice_discard_dismiss_after_late_mute_declines", base(32, 1, true), |p| {
            p.dismiss = true;
            p.response_table = true;
            p.late_mute = 1
        }),
        ("group_notice_mute_dismiss_declines", base(32, 2, true), |p| p.dismiss = true),
        ("group_notice_requested_accept_sends_33", base(37, 0, true), |_| {}),
        ("group_notice_requested_discard_dismiss", base(37, 1, true), |p| p.dismiss = true),
        ("group_notice_from_group_mute", base(32, 2, true), |p| {
            p.response_table = true;
            p.from_group = true;
            p.dismiss = true
        }),
    ];
    let mut out = Out::default();
    for (id, mut pc, set) in named {
        set(&mut pc);
        out.lines.extend(run_policy_case(&pc, Some(id)));
    }
    let direct: [(&str, &[u8], bool, bool, i32); 5] = [
        ("direct_group_notice_discard_dismiss_muted_resident", b"Grid Resident", true, true, 1),
        ("direct_group_notice_discard_dismiss_muted_linden", b"Grid Linden", true, true, 1),
        ("direct_group_notice_discard_dismiss_not_muted_linden", b"Grid Linden", false, true, 1),
        ("direct_group_notice_mute_dismiss_linden", b"Grid Linden", true, true, 2),
        ("direct_group_notice_closed_dismiss_muted", b"Grid Resident", true, true, 7),
    ];
    for (id, from_name, muted, dismiss, response) in direct {
        let mut f = setup();
        f.snapshots = true;
        f.settings_b.insert("FSDismissGroupNoticeAttachmentsToTrash".into(), dismiss);
        if muted {
            f.mute_insert(other(), from_name, 1, 0);
        }
        let mut sd = Value::Undefined;
        insert(&mut sd, "im_type", Value::Integer(32));
        insert(&mut sd, "from_id", Value::Uuid(other()));
        insert(&mut sd, "from_group", Value::Boolean(true));
        insert(&mut sd, "transaction_id", Value::Uuid(txn()));
        insert(&mut sd, "folder_id", Value::Uuid(uid(0xf0, 7)));
        insert(&mut sd, "type", Value::string("notecard"));
        insert(&mut sd, "from_name", Value::String(from_name.to_vec()));
        insert(&mut sd, "description", Value::string("Notice Item"));
        insert(&mut sd, "sender", Value::String(OFFER_HOST.to_vec()));
        put_world(&mut out, &f, id);
        f.add(format!("offer from LLSD {}", sd_notation(&sd)));
        let h = f.new_offer(OfferInfo::from_llsd(&sd));
        if response == 7 {
            let mut notification = Value::Undefined;
            insert(&mut notification, "name", Value::string("UserGiveItem"));
            insert(&mut notification, "form", notification_form("UserGiveItem").unwrap());
            insert(&mut notification, "id", Value::Uuid(uid(0x6e, 999)));
            f.respond_offer(h, &notification, &Value::Map(Default::default()));
        } else {
            f.force_response(h, response);
        }
        f.run_idle();
        if f.offers.contains_key(&h) {
            f.drop_offer_silently(h);
        }
        out.events(&mut f, id);
    }
    assert_lines("offer/policy_named", &compared("offer", "policy_named.txt"), &out.lines);
}
