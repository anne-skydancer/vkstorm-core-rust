//! Received offers (`Incoming`, `run_incoming`) and the offer policy runs
//! (`PolicyCase`, `run_policy_case`) of `llinventory_offer_oracle.cpp`.

use vkstorm_inventory::exchange::NetHost;
use vkstorm_inventory::exchange::names::stream_read_s32;
use vkstorm_inventory::exchange::offer::{self, ImArgs};
use vkstorm_inventory::perms::{NULL_ID, PERM_ALL, Uuid};
use vkstorm_llsd::Value;

use super::*;

pub const MTU: usize = 1200;

#[derive(Clone)]
pub struct Incoming {
    pub id: String,
    pub dialog: i32,
    pub bucket: Vec<u8>,
    pub declared_size: i32,
    pub session: Uuid,
    pub aux: Uuid,
    pub name: Vec<u8>,
    pub message: Vec<u8>,
    pub from_group: bool,
    pub from: Uuid,
    pub fill: i32,
    pub trim: bool,
}

impl Incoming {
    pub fn new(id: &str, dialog: i32, bucket: Vec<u8>) -> Incoming {
        Incoming {
            id: id.to_owned(),
            dialog,
            bucket,
            declared_size: -1,
            session: txn(),
            aux: NULL_ID,
            name: b"Sender Resident".to_vec(),
            message: b"Gift".to_vec(),
            from_group: false,
            from: other(),
            fill: 0,
            trim: true,
        }
    }
}

pub fn agent_bucket(ty: i8, id: &Uuid) -> Vec<u8> {
    let mut b = vec![ty as u8];
    b.extend_from_slice(id);
    b
}

pub fn notice_bucket(has_inv: u8, ty: u8, grp: &Uuid, name: &[u8], terminate: bool) -> Vec<u8> {
    let mut b = vec![has_inv, ty];
    b.extend_from_slice(grp);
    b.extend_from_slice(name);
    if terminate {
        b.push(0);
    }
    b
}

/// `legacy_type_probe(buffer)`.
pub fn legacy_type_probe(buffer: &[u8]) -> String {
    let s = &buffer[..buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len())];
    let t = &s[..s.iter().position(|&c| c == b'|').unwrap_or(s.len())];
    let v = stream_read_s32(t);
    format!(
        "legacy_read len={} type_text=\"{}\" {}",
        s.len(),
        esc(t),
        match v {
            None => "type=UNINITIALISED".to_owned(),
            Some(x) => format!("type={x}"),
        }
    )
}

/// `run_incoming(c, record_probe)`.
pub fn run_incoming(f: &mut Fake, c: &Incoming, record_probe: bool) {
    let mut buffer = vec![if c.fill != 0 { 0x41u8 } else { 0 }; MTU + 64];
    buffer[MTU + 63] = 0;
    if c.fill != 0 {
        buffer[MTU] = 0;
    }
    let n = c.bucket.len().min(MTU);
    buffer[..n].copy_from_slice(&c.bucket[..n]);
    let size = if c.declared_size >= 0 {
        c.declared_size
    } else {
        c.bucket.len() as i32
    };
    if record_probe && c.dialog == 9 && size != 1 {
        let p = legacy_type_probe(&buffer);
        f.add(p);
    }
    f.add(format!(
        "im dialog={} from={} group={} session={} aux={} size={} bucket={} name=\"{}\" message=\"{}\" fill={}",
        c.dialog,
        us(&c.from),
        c.from_group as i32,
        us(&c.session),
        us(&c.aux),
        size,
        hex(&c.bucket),
        esc(&c.name),
        esc(&c.message),
        c.fill
    ));
    let im = ImArgs {
        from_id: c.from,
        from_group: c.from_group,
        dialog: c.dialog,
        session_id: c.session,
        timestamp: 0,
        name: &c.name,
        message: &c.message,
        buffer: &buffer,
        bucket_size: size,
        sender: NetHost::parse(OFFER_HOST),
        aux_id: c.aux,
    };
    offer::receive_im(f, &im);
    f.run_idle();
}

// ============================================================ the policy

#[derive(Clone, Default)]
pub struct PolicyCase {
    pub dialog: i32,
    pub self_: bool,
    pub mute: i32,
    pub dnd: bool,
    pub auto_accept: bool,
    pub legacy: bool,
    pub rlv: bool,
    pub give_rlv: i32,
    pub forbid: bool,
    pub ty: i32,
    pub button: i32,
    pub forced: bool,
    pub dismiss: bool,
    pub log_decline: bool,
    pub present: bool,
    pub response_table: bool,
    pub from_group: bool,
    pub late_mute: i32,
    pub linden: bool,
    pub defer: bool,
}

impl PolicyCase {
    pub fn id(&self) -> String {
        const MUTES: [&str; 4] = ["none", "id", "name", "id_textchat_exempt"];
        const GRLV: [&str; 3] = ["no", "root", "noroot"];
        const TYPES: [&str; 3] = ["cat", "note", "bad"];
        const BUTTONS: [&str; 8] = [
            "accept",
            "decline",
            "mute",
            "show",
            "accept_silent",
            "decline_silent",
            "show_silent",
            "closed",
        ];
        let mut s = format!(
            "d{}.self{}.mute_{}.dnd{}.aa{}.legacy{}.rlv{}.grlv_{}.forbid{}.type_{}.{}.{}.dismiss{}.logdecl{}{}",
            self.dialog,
            self.self_ as i32,
            MUTES[self.mute as usize],
            self.dnd as i32,
            self.auto_accept as i32,
            self.legacy as i32,
            self.rlv as i32,
            GRLV[self.give_rlv as usize],
            self.forbid as i32,
            TYPES[self.ty as usize],
            BUTTONS[self.button as usize],
            if self.forced { "forced" } else { "interactive" },
            self.dismiss as i32,
            self.log_decline as i32,
            if self.present { ".present" } else { "" }
        );
        if self.response_table {
            let late = ["none", "id", "name"][self.late_mute as usize];
            let _ = write!(
                s,
                ".fg{}.late_{}.linden{}.defer{}",
                self.from_group as i32, late, self.linden as i32, self.defer as i32
            );
        }
        s
    }
}

const KEEP: [&str; 26] = [
    "send ",
    "notify.postponed ",
    "notify ",
    "notification.respond ",
    "model.removeObject ",
    "model.deleteObject ",
    "model.addObserver ",
    "fetch.start ",
    "fetch.done ",
    "open_inventory_offer ",
    "new LLOpenTaskGroupOffer",
    "new RlvGiveToRLV",
    "RlvGiveToRLVAgentOffer.done",
    "rlv.sendNotification ",
    "mute.add ",
    "mute.remove ",
    "recent_people.add ",
    "inventory_panel.select ",
    "group_panel.showNotice ",
    "stage ",
    "nearby_chat ",
    "toasts.kill ",
    "blocked_list.show ",
    "offer.",
    "responder.deleted",
    "name.deferred ",
];

pub fn policy_event(e: &str) -> bool {
    KEEP.iter().any(|k| e.starts_with(k))
}

/// `policy_render(e)`.
pub fn policy_render(e: &str) -> String {
    if e.starts_with("offer.") {
        let b = e.as_bytes();
        let mut out = String::new();
        let mut k = 0;
        while k < b.len() {
            let Some(sp) = e[k..].find(' ').map(|i| i + k) else {
                out.push_str(&e[k..]);
                break;
            };
            out.push_str(&e[k..sp]);
            let f = sp + 1;
            let rest = &e[f..];
            if rest.starts_with("from=") || rest.starts_with("from_object=") {
                k = rest.find(' ').map_or(e.len(), |i| i + f);
            } else if rest.starts_with("from_name=\"") || rest.starts_with("desc=\"") {
                let mut q = e[f..].find('"').unwrap() + f + 1;
                while q < b.len() && b[q] != b'"' {
                    q += if b[q] == b'\\' { 2 } else { 1 };
                }
                k = q + 1;
            } else {
                out.push(' ');
                k = f;
            }
        }
        return out;
    }
    if e.starts_with("send ") {
        if let Some(z) = e.find(" zc=") {
            return e[..z].to_owned();
        }
    }
    if e.starts_with("notify ") || e.starts_with("notify.postponed") {
        if let (Some(a), Some(p)) = (e.find(" args="), e.find(" payload=")) {
            if p > a {
                return format!("{}{}", &e[..a], &e[p..]);
            }
        }
    }
    e.to_owned()
}

/// `run_policy_case(pc, s, full_events, named)`: the arrival, the late mute,
/// the response; the projected events as `pNN` lines.
pub fn run_policy_case(pc: &PolicyCase, named: Option<&str>) -> Vec<String> {
    let mut f = setup();
    f.snapshots = true;
    f.defer = pc.defer;
    f.settings_b
        .insert("AutoAcceptNewInventory".into(), pc.auto_accept);
    f.settings_b
        .insert("FSUseLegacyInventoryAcceptMessages".into(), pc.legacy);
    f.settings_b
        .insert("FSDismissGroupNoticeAttachmentsToTrash".into(), pc.dismiss);
    f.settings_b
        .insert("LogInventoryDecline".into(), pc.log_decline);
    f.dnd = pc.dnd;
    f.rlv_enabled = pc.rlv;
    f.rlv_forbid = pc.forbid;
    f.rlv_shared_root = if pc.give_rlv == 1 {
        rlv_root()
    } else {
        NULL_ID
    };
    let agent_offer = pc.dialog == 4;
    let task = pc.dialog == 9;
    let from = if task && pc.from_group {
        group()
    } else if pc.self_ {
        agent()
    } else {
        other()
    };
    let name: &[u8] = if pc.linden {
        b"Grid Linden"
    } else if task {
        b"Gadget"
    } else {
        b"Sender"
    };
    match pc.mute {
        1 => f.mute_insert(from, name, 1, 0),
        2 => {
            f.legacy_mutes.insert(name.to_vec());
        }
        3 => f.mute_insert(from, name, 1, 1),
        _ => {}
    }
    let ty = match pc.ty {
        0 => 8,
        1 => 7,
        _ => 127,
    };
    if pc.present {
        if ty == 8 {
            f.add_cat(offered(), root(), b"Offered", -1);
        } else {
            f.add_item(
                offered(),
                root(),
                ty,
                &perms_simple(&agent(), PERM_ALL),
                b"Offered",
                None,
            );
        }
    }
    let mut c = Incoming::new("", pc.dialog, Vec::new());
    c.from = from;
    c.from_group = pc.from_group;
    c.name = name.to_vec();
    c.session = txn();
    if agent_offer {
        c.bucket = agent_bucket(ty as i8, &offered());
        c.message = if pc.give_rlv != 0 {
            b"#RLV/~kit".to_vec()
        } else {
            b"Gift".to_vec()
        };
    } else if task {
        c.bucket = vec![ty as u8];
        c.message = if pc.give_rlv != 0 {
            b"'#RLV/~kit'  ( http://slurl.com/secondlife/Region/1/2/3 )".to_vec()
        } else {
            b"'Gadget Item'  ( http://slurl.com/secondlife/Region/1/2/3 )".to_vec()
        };
    } else {
        let nm: &[u8] = if pc.give_rlv != 0 {
            b"#RLV/~kit"
        } else {
            b"Notice Item"
        };
        c.bucket = notice_bucket(1, ty as u8, &group(), nm, true);
        c.message = b"Subject|Body".to_vec();
    }
    f.ev.clear();
    f.add("stage arrival");
    run_incoming(&mut f, &c, false);
    if pc.late_mute != 0 {
        f.add(format!(
            "stage late_mute {}",
            if pc.late_mute == 1 {
                "by_id"
            } else {
                "by_name"
            }
        ));
        if pc.late_mute == 1 {
            f.mute_insert(from, name, 1, 0);
        } else {
            f.legacy_mutes.insert(name.to_vec());
        }
    }
    f.add(format!(
        "stage response {} button={}",
        if pc.forced { "forced" } else { "interactive" },
        pc.button
    ));
    if pc.button == 7 && pc.forced {
        f.add("stage unreachable (no forced close)");
    } else if agent_offer || task {
        match f.posted.last().copied() {
            None => f.add("stage none (no notification posted)"),
            Some(nid) => {
                let n = f.live.get(&nid).cloned();
                let info = n
                    .as_ref()
                    .and_then(|n| n.responder)
                    .filter(|h| f.offers.contains_key(h));
                match (n, info) {
                    (Some(n), Some(h)) => {
                        if pc.forced {
                            f.force_response(h, pc.button);
                        } else {
                            let mut found = pc.button == 7;
                            let response = if pc.button == 7 {
                                vkstorm_inventory::exchange::notify::response_template(
                                    n.form.as_ref().unwrap_or(&Value::Undefined),
                                )
                            } else {
                                let mut r = Value::Undefined;
                                if let Some(Value::Array(a)) = &n.form {
                                    for e in a {
                                        if vkstorm_inventory::llsd::coerce::as_integer(get(
                                            e, "index",
                                        )) == pc.button
                                        {
                                            let k =
                                                String::from_utf8_lossy(&as_string(get(e, "name")))
                                                    .into_owned();
                                            insert(&mut r, &k, Value::Boolean(true));
                                            found = true;
                                        }
                                    }
                                }
                                r
                            };
                            if found {
                                f.respond_notification(&nid, &response);
                            } else {
                                f.add(format!("stage unreachable ({} has no such button)", n.name));
                            }
                        }
                    }
                    _ => f.add("stage none (no responder)"),
                }
            }
        }
    } else {
        let mut info: Option<u32> = None;
        if pc.dialog == 32
            && vkstorm_inventory::llsd::coerce::has(&f.group_notice_payload, "inventory_offer")
        {
            let o = OfferInfo::from_llsd(get(&f.group_notice_payload, "inventory_offer"));
            info = Some(f.new_offer(o));
        } else if pc.dialog == 37 {
            info = f.group_panel_offers.get(&group()).copied().flatten();
            f.group_panel_offers.clear();
        }
        if !pc.forced {
            f.add("stage unreachable (group notice offers are answered by forced responses)");
            if let Some(h) = info {
                f.drop_offer_silently(h);
            }
        } else if let Some(h) = info {
            f.force_response(h, pc.button);
            if f.offers.contains_key(&h) {
                f.drop_offer_silently(h);
            }
        } else {
            f.add("stage none (no offer)");
        }
    }
    f.run_idle();
    let cid = named.map_or_else(|| pc.id(), str::to_owned);
    let mut out = Vec::new();
    let mut n = 0;
    for e in &f.ev {
        if policy_event(e) {
            out.push(format!("{cid}\tp{n:02}\t{}", policy_render(e)));
            n += 1;
        }
    }
    out
}

/// `for_each_policy_case`.
pub fn for_each_policy_case(mut f: impl FnMut(&PolicyCase)) {
    let mut pc = PolicyCase::default();
    for d in [4, 9, 32, 37] {
        for self_ in [false, true] {
            for mute in 0..4 {
                for dnd in [false, true] {
                    for aa in [false, true] {
                        for legacy in [false, true] {
                            for rlv in [false, true] {
                                for grlv in 0..3 {
                                    for forbid in [false, true] {
                                        for ty in 0..3 {
                                            for button in 0..8 {
                                                for forced in [false, true] {
                                                    for dismiss in [false, true] {
                                                        for logdecl in [false, true] {
                                                            pc.dialog = d;
                                                            pc.self_ = self_;
                                                            pc.mute = mute;
                                                            pc.dnd = dnd;
                                                            pc.auto_accept = aa;
                                                            pc.legacy = legacy;
                                                            pc.rlv = rlv;
                                                            pc.give_rlv = grlv;
                                                            pc.forbid = forbid;
                                                            pc.ty = ty;
                                                            pc.button = button;
                                                            pc.forced = forced;
                                                            pc.dismiss = dismiss;
                                                            pc.log_decline = logdecl;
                                                            f(&pc);
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

/// `for_each_response_case`.
pub fn for_each_response_case(mut f: impl FnMut(&PolicyCase)) {
    let mut pc = PolicyCase {
        response_table: true,
        ..PolicyCase::default()
    };
    for (d, fg) in [
        (4, false),
        (9, false),
        (9, true),
        (32, false),
        (32, true),
        (37, false),
        (37, true),
    ] {
        for self_ in [false, true] {
            for linden in [false, true] {
                for late in 0..3 {
                    for defer in [false, true] {
                        for legacy in [false, true] {
                            for ty in 0..2 {
                                for button in 0..8 {
                                    for forced in [false, true] {
                                        for dismiss in [false, true] {
                                            for logdecl in [false, true] {
                                                pc.dialog = d;
                                                pc.from_group = fg;
                                                pc.self_ = self_;
                                                pc.linden = linden;
                                                pc.late_mute = late;
                                                pc.defer = defer;
                                                pc.legacy = legacy;
                                                pc.ty = ty;
                                                pc.button = button;
                                                pc.forced = forced;
                                                pc.dismiss = dismiss;
                                                pc.log_decline = logdecl;
                                                f(&pc);
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

/// `for` the present table.
pub fn for_each_present_case(mut f: impl FnMut(&PolicyCase)) {
    let mut pc = PolicyCase {
        present: true,
        log_decline: true,
        ..PolicyCase::default()
    };
    for d in [4, 9, 32, 37] {
        for ty in 0..2 {
            for aa in [false, true] {
                for legacy in [false, true] {
                    for button in 0..8 {
                        for forced in [false, true] {
                            pc.dialog = d;
                            pc.ty = ty;
                            pc.auto_accept = aa;
                            pc.legacy = legacy;
                            pc.button = button;
                            pc.forced = forced;
                            f(&pc);
                        }
                    }
                }
            }
        }
    }
}
