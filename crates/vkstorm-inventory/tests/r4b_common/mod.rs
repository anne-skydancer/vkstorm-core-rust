//! The R4b replay harness: this crate's counterpart of the oracle's fakes
//! (`llinventory_offer_oracle_fakes.{h,cpp}`) and of the corpus helpers of
//! `llinventory_offer_oracle.cpp`. [`Fake`] holds the world the decisions
//! read and implements [`Host`]: every [`Action`] is carried out here and
//! logged as the C++ fakes log it, so the event lines a case produces can be
//! compared with the fixtures (`fixtures/offer/README.md`, "Events").
//!
//! Packets are built with `vkstorm-messages` over the viewer's
//! `message_template.msg`; LLSD is serialised with `vkstorm-llsd`.
#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use vkstorm_inventory::exchange::give::{self, ItemFilter};
use vkstorm_inventory::exchange::host::{
    Action, Handler, Host, ImPacket, ModelCat, ModelItem, MoveInv, NameThen, ObserverId, ObserverKind, OfflineMessage, RlvBehaviour,
    TaskObject, ViewerObject,
};
use vkstorm_inventory::exchange::notify::selected_option;
use vkstorm_inventory::exchange::{offer, NetHost, OfferInfo, Throttle};
use vkstorm_inventory::llsd::coerce::{as_string, as_uuid, get, insert};
use vkstorm_inventory::perms::{Permissions, Uuid, NULL_ID, PERM_ALL, PERM_COPY, PERM_MODIFY, PERM_MOVE, PERM_NONE, PERM_TRANSFER};
use vkstorm_inventory::text::correct_inventory_name;
use vkstorm_inventory::types::{AssetType, FolderType, InventoryType};
use vkstorm_llsd::{Format, Options, Value};
use vkstorm_messages::{parse_template, Builder, Template, Tokenizer, VarType, MAX_BUFFER_SIZE, PACKET_ID_SIZE};

// ===================================================================== text

/// `esc`: bytes 0x20-0x7e as they are except `\` as `\\`; others `\xHH`.
pub fn esc(s: &[u8]) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for &c in s {
        match c {
            b'\\' => out.push_str("\\\\"),
            0x20..=0x7e => out.push(c as char),
            _ => {
                let _ = write!(out, "\\x{c:02x}");
            }
        }
    }
    out
}

pub fn hex(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len() * 2);
    for b in data {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// `LLUUID::asString()`.
pub fn us(id: &Uuid) -> String {
    let h = hex(id);
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

/// `U(tag, n)`: `%08x-0000-4000-8000-%012x`.
pub fn uid(tag: u32, n: u64) -> Uuid {
    let mut id = [0u8; 16];
    id[..4].copy_from_slice(&tag.to_be_bytes());
    id[6] = 0x40;
    id[8] = 0x80;
    id[10..].copy_from_slice(&n.to_be_bytes()[2..]);
    id
}

pub fn sd_bin_hex(v: &Value) -> String {
    hex(&vkstorm_llsd::format(Format::Binary, v, Options::default()).0)
}

pub fn sd_notation(v: &Value) -> String {
    let opts = Options { pretty_binary: false, ..Options::default() };
    esc(&vkstorm_llsd::format(Format::Notation, v, opts).0)
}

pub fn ids(v: &[Uuid]) -> String {
    let parts: Vec<String> = v.iter().map(us).collect();
    format!("[{}]", parts.join(","))
}

pub fn id_set(v: &BTreeSet<Uuid>) -> String {
    let parts: Vec<String> = v.iter().map(us).collect();
    format!("[{}]", parts.join(","))
}

/// `%g` of an `F32` widened to double (only the values the harness uses).
pub fn g(v: f32) -> String {
    let mut out = Vec::new();
    vkstorm_llsd::text::fmt_g(v as f64, 6, &mut out);
    String::from_utf8(out).unwrap()
}

// ================================================================ fixed ids

pub const AGENT_TAG: u32 = 0xa1;
pub fn agent() -> Uuid { uid(0xa1, 1) }
pub fn session() -> Uuid { uid(0xa1, 2) }
pub fn other() -> Uuid { uid(0xb2, 1) }
pub fn other2() -> Uuid { uid(0xb2, 2) }
pub fn creator() -> Uuid { uid(0xb2, 9) }
pub fn group() -> Uuid { uid(0xc3, 1) }
pub fn txn() -> Uuid { uid(0xd4, 1) }
pub fn offered() -> Uuid { uid(0xe5, 1) }
pub fn rlv_root() -> Uuid { uid(0xf1, 1) }
pub fn root() -> Uuid { uid(0x1f, 1) }
pub fn folder_for(ft: i32) -> Uuid { uid(0xf0, ft as u16 as u64) }
pub const OFFER_HOST: &[u8] = b"10.0.0.9:13005";
pub const AGENT_HOST: &[u8] = b"10.0.0.1:13000";

pub fn perm_mask(copy: bool, modify: bool, transfer: bool) -> u32 {
    PERM_MOVE | if copy { PERM_COPY } else { 0 } | if modify { PERM_MODIFY } else { 0 } | if transfer { PERM_TRANSFER } else { 0 }
}

/// `make_perms(owner, owner_mask, everyone, base, group, group_owned)`.
pub fn make_perms(owner: &Uuid, owner_mask: u32, everyone: u32, base: u32, grp: &Uuid, group_owned: bool) -> Permissions {
    let mut p = Permissions::default();
    p.init(&creator(), owner, owner, grp);
    p.init_masks(base, owner_mask, everyone, PERM_NONE, PERM_ALL);
    if group_owned {
        p.set_owner_and_group(&NULL_ID, &NULL_ID, grp, true);
    }
    p
}

pub fn perms_simple(owner: &Uuid, owner_mask: u32) -> Permissions {
    make_perms(owner, owner_mask, PERM_NONE, PERM_ALL, &NULL_ID, false)
}

// ============================================================ the template

pub struct Messages {
    pub iim: Arc<Template>,
    pub builder: Builder,
}

fn template() -> &'static Arc<Template> {
    static T: OnceLock<Arc<Template>> = OnceLock::new();
    T.get_or_init(|| {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../vkstorm-messages/tests/fixtures/message_template.msg");
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let file = parse_template(&mut Tokenizer::new(&text)).expect("message template parses");
        let t = file.messages.into_iter().find(|t| t.name == "ImprovedInstantMessage").expect("ImprovedInstantMessage");
        Arc::new(t.with_index_keys())
    })
}

/// The `ImprovedInstantMessage` as `buildMessage` lays it out (header bytes
/// 0-4 zero) and its zero-coded form (`None`: coding would not shrink it).
pub fn build_im(p: &ImPacket) -> (Vec<u8>, Option<Vec<u8>>) {
    let t = template().clone();
    let mut b = Builder::new();
    b.new_message(t.clone());
    let key = |block: &str, var: &str| t.block_by_name(block).unwrap().variable_by_name(var).unwrap().key;
    let bk = |block: &str| t.block_by_name(block).unwrap().key;
    b.next_block(bk("AgentData")).unwrap();
    b.add_data(key("AgentData", "AgentID"), &p.agent_id, VarType::LLUUID).unwrap();
    b.add_data(key("AgentData", "SessionID"), &p.session_id, VarType::LLUUID).unwrap();
    b.next_block(bk("MessageBlock")).unwrap();
    let mb = |v: &str| key("MessageBlock", v);
    b.add_data(mb("FromGroup"), &[u8::from(p.from_group)], VarType::Bool).unwrap();
    b.add_data(mb("ToAgentID"), &p.to_agent_id, VarType::LLUUID).unwrap();
    b.add_data(mb("ParentEstateID"), &p.parent_estate_id.to_le_bytes(), VarType::U32).unwrap();
    b.add_data(mb("RegionID"), &p.region_id, VarType::LLUUID).unwrap();
    let mut pos = Vec::with_capacity(12);
    for c in p.position {
        pos.extend_from_slice(&c.to_le_bytes());
    }
    b.add_data(mb("Position"), &pos, VarType::LLVector3).unwrap();
    b.add_data(mb("Offline"), &[p.offline], VarType::U8).unwrap();
    b.add_data(mb("Dialog"), &[p.dialog], VarType::U8).unwrap();
    b.add_data(mb("ID"), &p.id, VarType::LLUUID).unwrap();
    b.add_data(mb("Timestamp"), &p.timestamp.to_le_bytes(), VarType::U32).unwrap();
    b.add_data(mb("FromAgentName"), &p.from_agent_name, VarType::Variable).unwrap();
    b.add_data(mb("Message"), &p.message, VarType::Variable).unwrap();
    b.add_data(mb("BinaryBucket"), &p.binary_bucket, VarType::Variable).unwrap();
    let mut buf = vec![0u8; MAX_BUFFER_SIZE];
    let n = b.build_message(&mut buf, 0).expect("message builds");
    buf.truncate(n);
    for x in buf.iter_mut().take(5) {
        *x = 0;
    }
    let mut coded = vec![0u8; buf.len() * 2 + 16];
    let zc = vkstorm_messages::zerocode::zero_code(&buf, &mut coded).filter(|&n| n > 0).map(|n| coded[..n].to_vec());
    let _ = PACKET_ID_SIZE;
    (buf, zc)
}

// ===================================================== notification forms

/// The frozen button table (`R4B-FORMS`): (notification, name, index,
/// default).
const FORMS: &[(&str, &str, i32, bool)] = &[
    ("ObjectGiveItem", "Keep", 0, false),
    ("ObjectGiveItem", "Discard", 1, false),
    ("ObjectGiveItem", "Mute", 2, false),
    ("OwnObjectGiveItem", "Keep", 0, false),
    ("OwnObjectGiveItem", "Discard", 1, false),
    ("UserGiveItem", "Show", 3, false),
    ("UserGiveItem", "Keep", 0, false),
    ("UserGiveItem", "Discard", 1, false),
    ("UserGiveItem", "Mute", 2, false),
    ("UserGiveItemLegacy", "Show", 3, false),
    ("UserGiveItemLegacy", "Accept", 0, false),
    ("UserGiveItemLegacy", "Discard", 1, false),
    ("UserGiveItemLegacy", "ShowSilent", 6, false),
    ("UserGiveItemLegacy", "AcceptSilent", 4, false),
    ("UserGiveItemLegacy", "DiscardSilent", 5, false),
    ("UserGiveItemLegacy", "Mute", 2, false),
    ("CannotCopyWarning", "OK_okcancelbuttons", 0, true),
    ("CannotCopyWarning", "Cancel_okcancelbuttons", 1, false),
    ("CannotCopyCountItems", "OK_okcancelbuttons", 0, true),
    ("CannotCopyCountItems", "Cancel_okcancelbuttons", 1, false),
    ("MoveInventoryFromObject", "OK_okcancelignore", 0, true),
    ("MoveInventoryFromObject", "Cancel_okcancelignore", 1, false),
    ("ShareItemsConfirmation", "OK_okcancelignore", 0, true),
    ("ShareItemsConfirmation", "Cancel_okcancelignore", 1, false),
];

/// `notification_form(name)`: the buttons as form elements, `None` when the
/// template has none (`LLSD()`).
pub fn notification_form(name: &str) -> Option<Value> {
    let mut form = Vec::new();
    for &(n, b, i, d) in FORMS {
        if n != name {
            continue;
        }
        let mut e = Value::Undefined;
        insert(&mut e, "type", Value::string("button"));
        insert(&mut e, "name", Value::string(b));
        insert(&mut e, "index", Value::Integer(i));
        insert(&mut e, "text", Value::string(b));
        if d {
            insert(&mut e, "default", Value::Boolean(true));
        }
        form.push(e);
    }
    (!form.is_empty()).then_some(Value::Array(form))
}

/// `respond`'s `selected=`: the true booleans of the response, in key order.
pub fn selected(response: &Value) -> String {
    let mut s = Vec::new();
    if let Value::Map(m) = response {
        for (k, v) in m {
            if matches!(v, Value::Boolean(true)) {
                s.push(String::from_utf8_lossy(k).into_owned());
            }
        }
    }
    if s.is_empty() { "-".into() } else { s.join(",") }
}

// ================================================================ the world

#[derive(Clone, Debug)]
pub struct Item {
    pub rec: ModelItem,
    pub inv_type: i32,
    pub flags: u32,
}

#[derive(Clone, Debug)]
pub struct Notif {
    pub name: String,
    pub id: Uuid,
    pub payload: Value,
    pub substitutions: Value,
    pub form: Option<Value>,
    pub handler: Option<Handler>,
    pub responder: Option<u32>,
}

impl Notif {
    /// `LLNotification::asLLSD()`.
    pub fn as_llsd(&self) -> Value {
        let mut sd = Value::Undefined;
        insert(&mut sd, "name", Value::string(&self.name));
        insert(&mut sd, "id", Value::Uuid(self.id));
        insert(&mut sd, "form", self.form.clone().unwrap_or(Value::Array(Vec::new())));
        insert(&mut sd, "payload", self.payload.clone());
        insert(&mut sd, "substitutions", self.substitutions.clone());
        sd
    }
}

#[derive(Clone, Debug)]
pub struct Obs {
    pub kind: ObserverKind,
    pub complete: Vec<Uuid>,
    pub incomplete: Vec<Uuid>,
}

impl Obs {
    fn class(&self) -> &'static str {
        match self.kind {
            ObserverKind::FetchItems { .. } => "LLInventoryFetchItemsObserver",
            ObserverKind::OpenAgentOffer { .. } => "LLOpenAgentOffer",
            ObserverKind::DiscardAgentOffer { .. } => "LLDiscardAgentOffer",
            ObserverKind::RlvGiveToRlvAgentOffer { .. } => "RlvGiveToRLVAgentOffer",
            ObserverKind::RlvGiveToRlvTaskOffer { .. } => "RlvGiveToRLVTaskOffer",
            ObserverKind::OpenTaskGroupOffer => "LLOpenTaskGroupOffer",
        }
    }
}

pub struct Postponed {
    pub name: String,
    pub substitutions: Value,
    pub payload: Value,
    pub offer_from_agent: bool,
    pub from_id: Uuid,
    pub is_group: bool,
    pub responder: Option<u32>,
}

pub enum Idle {
    Name { id: Uuid, name: Vec<u8>, is_group: bool, then: NameThen },
    Postponed(Postponed),
    RemoveObject(Uuid),
    MainCoro(OfflineMessage),
}

#[derive(Clone, Debug)]
pub struct Mute {
    pub id: Uuid,
    pub name: Vec<u8>,
    pub ty: i32,
    pub flags: u32,
}

pub struct Pending {
    pub name: String,
    pub payload: Value,
    pub handler: Handler,
}

pub struct ObjectRec {
    pub obj: ViewerObject,
    pub inventory: Vec<TaskObject>,
}

/// The fakes' state (`World`, the singletons, the model) and the event log.
pub struct Fake {
    pub ev: Vec<String>,
    pub on: bool,
    // agent
    pub agent: Uuid,
    pub session: Uuid,
    pub agent_group: Uuid,
    pub position: [f32; 3],
    pub dnd: bool,
    pub region_host: NetHost,
    pub groups: BTreeSet<Uuid>,
    pub have_avatar: bool,
    pub avatar_valid: bool,
    pub fullname: Vec<u8>,
    pub attachments: BTreeSet<Uuid>,
    pub sitting: bool,
    pub avatar_root: Option<Uuid>,
    pub wearables: BTreeSet<Uuid>,
    pub gestures: BTreeSet<Uuid>,
    pub settings_b: BTreeMap<String, bool>,
    pub settings_u: BTreeMap<String, u32>,
    pub dnd_response: Vec<u8>,
    pub trim_resident: bool,
    // model
    pub items: BTreeMap<Uuid, Item>,
    pub cat_records: BTreeMap<Uuid, ModelCat>,
    pub cat_map: BTreeSet<Uuid>,
    pub cat_tree: BTreeMap<Uuid, Vec<Uuid>>,
    pub item_tree: BTreeMap<Uuid, Vec<Uuid>>,
    pub incomplete: BTreeSet<Uuid>,
    pub system_folders: BTreeMap<i32, Uuid>,
    pub observers: Vec<Obs>,
    // mutes, names
    pub mutes: BTreeMap<Uuid, Mute>,
    pub legacy_mutes: BTreeSet<Vec<u8>>,
    pub avatar_names: BTreeMap<Uuid, Vec<u8>>,
    pub ids_by_legacy_name: BTreeMap<Vec<u8>, Uuid>,
    pub group_names: BTreeMap<Uuid, Vec<u8>>,
    pub im_sessions: BTreeSet<Uuid>,
    // notifications
    pub live: BTreeMap<Uuid, Notif>,
    pub notification_counter: u64,
    pub pending: Vec<Pending>,
    pub posted: Vec<Uuid>,
    pub group_notice_payload: Value,
    pub group_panel_offers: BTreeMap<Uuid, Option<u32>>,
    // offers
    pub offers: BTreeMap<u32, OfferInfo>,
    pub next_offer: u32,
    pub cur: Option<u32>,
    pub snapshots: bool,
    // world
    pub objects: BTreeMap<Uuid, ObjectRec>,
    pub generate_counter: u64,
    pub idle: VecDeque<Idle>,
    pub defer: bool,
    pub startup_started: bool,
    pub max_transfer: i32,
    pub throttle: Throttle,
    pub clock: f64,
    pub packed: Option<ImPacket>,
    // RLVa
    pub rlv_enabled: bool,
    pub rlv_forbid: bool,
    pub rlv_shared_root: Uuid,
    pub rlv_can_show_names: bool,
    pub rlv_share_blocked: BTreeSet<Uuid>,
    pub rlv_nearby: BTreeSet<Uuid>,
    pub rlv_locked_attachment: bool,
    pub rlv_unsit: bool,
    pub rlv_sittp: bool,
}

impl Default for Fake {
    fn default() -> Self {
        Fake {
            ev: Vec::new(),
            on: true,
            agent: NULL_ID,
            session: NULL_ID,
            agent_group: NULL_ID,
            position: [0.0; 3],
            dnd: false,
            region_host: NetHost::default(),
            groups: BTreeSet::new(),
            have_avatar: true,
            avatar_valid: true,
            fullname: Vec::new(),
            attachments: BTreeSet::new(),
            sitting: false,
            avatar_root: None,
            wearables: BTreeSet::new(),
            gestures: BTreeSet::new(),
            settings_b: BTreeMap::new(),
            settings_u: BTreeMap::new(),
            dnd_response: Vec::new(),
            trim_resident: true,
            items: BTreeMap::new(),
            cat_records: BTreeMap::new(),
            cat_map: BTreeSet::new(),
            cat_tree: BTreeMap::new(),
            item_tree: BTreeMap::new(),
            incomplete: BTreeSet::new(),
            system_folders: BTreeMap::new(),
            observers: Vec::new(),
            mutes: BTreeMap::new(),
            legacy_mutes: BTreeSet::new(),
            avatar_names: BTreeMap::new(),
            ids_by_legacy_name: BTreeMap::new(),
            group_names: BTreeMap::new(),
            im_sessions: BTreeSet::new(),
            live: BTreeMap::new(),
            notification_counter: 0,
            pending: Vec::new(),
            posted: Vec::new(),
            group_notice_payload: Value::Undefined,
            group_panel_offers: BTreeMap::new(),
            offers: BTreeMap::new(),
            next_offer: 0,
            cur: None,
            snapshots: false,
            objects: BTreeMap::new(),
            generate_counter: 0,
            idle: VecDeque::new(),
            defer: false,
            startup_started: true,
            max_transfer: 42,
            throttle: Throttle::default(),
            clock: 1000.0,
            packed: None,
            rlv_enabled: false,
            rlv_forbid: false,
            rlv_shared_root: NULL_ID,
            rlv_can_show_names: true,
            rlv_share_blocked: BTreeSet::new(),
            rlv_nearby: BTreeSet::new(),
            rlv_locked_attachment: false,
            rlv_unsit: false,
            rlv_sittp: false,
        }
    }
}

/// `offer_fields(o)`.
pub fn offer_fields(o: &OfferInfo) -> String {
    format!(
        "im={} from={} from_group={} from_object={} txn={} folder={} object={} type={} from_name=\"{}\" desc=\"{}\" host={} persist={}",
        o.im,
        us(&o.from_id),
        o.from_group as i32,
        o.from_object as i32,
        us(&o.transaction_id),
        us(&o.folder_id),
        us(&o.object_id),
        o.ty.0,
        esc(&o.from_name),
        esc(&o.desc),
        o.host.ip_and_port(),
        o.persist as i32
    )
}

fn short(id: &Uuid) -> String {
    us(id)[..8].to_owned()
}

fn move_list(m: &MoveInv) -> String {
    let parts: Vec<String> = m.moves.iter().map(|(a, b)| format!("{}>{}", us(a), us(b))).collect();
    format!("object={} category={} list=[{}]", us(&m.object_id), us(&m.category_id), parts.join(","))
}

/// `llformat(format, arg)` for a format whose only conversion is `%s`.
fn llformat_s(format: &[u8], arg: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    let mut used = false;
    while i < format.len() {
        if format[i] == b'%' && i + 1 < format.len() {
            match format[i + 1] {
                b's' if !used => {
                    out.extend_from_slice(arg);
                    used = true;
                    i += 2;
                    continue;
                }
                b'%' => {
                    out.push(b'%');
                    i += 2;
                    continue;
                }
                _ => {}
            }
        }
        out.push(format[i]);
        i += 1;
    }
    out
}

impl Fake {
    pub fn add(&mut self, line: impl Into<String>) {
        if self.on {
            self.ev.push(line.into());
        }
    }

    // --------------------------------------------------------------- model
    pub fn add_cat(&mut self, id: Uuid, parent: Uuid, name: &[u8], pref: i32) -> ModelCat {
        let c = ModelCat { id, parent, preferred: FolderType(pref), name: correct_inventory_name(name) };
        self.cat_records.insert(id, c.clone());
        self.cat_map.insert(id);
        self.cat_tree.entry(parent).or_default().push(id);
        c
    }

    pub fn add_item(&mut self, id: Uuid, parent: Uuid, ty: i32, perm: &Permissions, name: &[u8], asset: Option<Uuid>) -> ModelItem {
        let inv = InventoryType::default_for_asset_type(AssetType(ty));
        let mut p = *perm;
        p.init_masks_for_type(inv);
        let asset = asset.unwrap_or_else(|| uid(0xaa, (id[15] as u64) | ((id[14] as u64) << 8)));
        let rec = ModelItem { id, parent, ty: AssetType(ty), asset, name: correct_inventory_name(name), perm: p, finished: true };
        self.items.insert(id, Item { rec: rec.clone(), inv_type: inv.0, flags: 0 });
        self.item_tree.entry(parent).or_default().push(id);
        rec
    }

    /// A new item record that is not added to the model.
    pub fn loose_item(id: Uuid, parent: Uuid, ty: i32, perm: &Permissions, name: &[u8], asset: Uuid, inv_type: i32) -> Item {
        let mut p = *perm;
        p.init_masks_for_type(InventoryType(inv_type));
        Item { rec: ModelItem { id, parent, ty: AssetType(ty), asset, name: correct_inventory_name(name), perm: p, finished: true }, inv_type, flags: 0 }
    }

    pub fn insert_item(&mut self, it: Item) {
        let parent = it.rec.parent;
        let id = it.rec.id;
        self.items.insert(id, it);
        self.item_tree.entry(parent).or_default().push(id);
    }

    pub fn set_item_raw_name(&mut self, id: &Uuid, name: &[u8]) {
        self.items.get_mut(id).unwrap().rec.name = name.to_vec();
    }

    pub fn set_cat_raw_name(&mut self, id: &Uuid, name: &[u8]) {
        self.cat_records.get_mut(id).unwrap().name = name.to_vec();
    }

    pub fn item_rec_line(&self, it: &Item) -> String {
        let r = &it.rec;
        format!(
            "item {} parent={} type={} inv_type={} asset={} flags={} finished={} name=\"{}\" {}",
            us(&r.id),
            us(&r.parent),
            r.ty.0,
            it.inv_type,
            us(&r.asset),
            it.flags,
            r.finished as i32,
            esc(&r.name),
            perm_rec(&r.perm)
        )
    }

    pub fn cat_rec_line(&self, c: &ModelCat) -> String {
        format!(
            "cat {} parent={} pref={} complete={} name=\"{}\"",
            us(&c.id),
            us(&c.parent),
            c.preferred.0,
            (!self.incomplete.contains(&c.id)) as i32,
            esc(&c.name)
        )
    }

    // -------------------------------------------------------------- mutes
    /// `LLMuteList::isMuted(id, name, flags)` (the frozen copy).
    pub fn muted(&self, id: &Uuid, name: &[u8], flags: u32) -> bool {
        if self.mutes.is_empty() && self.legacy_mutes.is_empty() {
            return false;
        }
        if *id == self.agent {
            return false;
        }
        if let Some(m) = self.mutes.get(id) {
            return flags & m.flags == 0;
        }
        if name.is_empty() {
            return false;
        }
        self.legacy_mutes.contains(name)
    }

    pub fn mute_insert(&mut self, id: Uuid, name: &[u8], ty: i32, flags: u32) {
        self.mutes.entry(id).or_insert(Mute { id, name: name.to_vec(), ty, flags });
    }

    // ------------------------------------------------------- notifications
    fn new_notification_id(&mut self) -> Uuid {
        self.notification_counter += 1;
        uid(0x6e00_0000, self.notification_counter)
    }

    /// The live offers' state, sorted (`install_snapshot_hook`).
    pub fn snapshot(&mut self, where_: &str) {
        if !self.snapshots {
            return;
        }
        let mut snaps: Vec<String> = self.offers.values().map(offer_fields).collect();
        snaps.sort();
        for s in snaps {
            self.add(format!("offer.{where_} {s}"));
        }
    }

    fn name_answer(&mut self, what: String, id: Uuid, name: Vec<u8>, is_group: bool, then: NameThen) {
        if self.defer {
            self.add(format!("name.deferred {what}"));
            self.idle.push_back(Idle::Name { id, name, is_group, then });
        } else {
            self.run_name_then(id, name, is_group, then);
        }
    }

    fn avatar_lookup_name(&self, id: &Uuid) -> Vec<u8> {
        self.avatar_names.get(id).cloned().unwrap_or_else(|| format!("resolved.{}", short(id)).into_bytes())
    }

    fn group_lookup_name(&self, id: &Uuid) -> Vec<u8> {
        self.group_names.get(id).cloned().unwrap_or_else(|| format!("Group {}", short(id)).into_bytes())
    }

    fn run_name_then(&mut self, id: Uuid, name: Vec<u8>, is_group: bool, then: NameThen) {
        match then {
            NameThen::OfferMuteAvatar => self.offer_mute(id, name, false),
            NameThen::OfferMuteGroup => self.offer_mute(id, name, is_group),
            NameThen::SystemMessageTip { format, message } => {
                let mut msg = llformat_s(&format, &name);
                msg.extend_from_slice(&message);
                let mut args = Value::Undefined;
                insert(&mut args, "MESSAGE", Value::String(msg));
                self.act(Action::Notify { name: "SystemMessageTip".into(), substitutions: args, payload: Value::Undefined, handler: None });
            }
            NameThen::GroupNotice { sender, subject, message, payload, timestamp } => {
                self.add(format!(
                    "notify GroupNotice group_name=\"{}\" sender=\"{}\" subject=\"{}\" message=\"{}\" timestamp={} payload={}",
                    esc(&name),
                    esc(&sender),
                    esc(&subject),
                    esc(&message),
                    timestamp,
                    sd_bin_hex(&payload)
                ));
                self.group_notice_payload = payload;
            }
            NameThen::ShareConfirmation { item_id, item_name, dest } => {
                give::show_object_sharing_confirmation(self, &name, &item_id, &item_name, &dest, &id, &NULL_ID);
            }
            NameThen::AutoUnmuteNotice { reason } => {
                self.add(format!("mute.auto_unmute_notice id={} name=\"{}\" reason={}", us(&id), esc(&name), reason));
            }
        }
    }

    /// `inventory_offer_mute_callback(blocked_id, full_name, is_group)`.
    fn offer_mute(&mut self, blocked: Uuid, name: Vec<u8>, is_group: bool) {
        let ty = if is_group { 3 } else { 1 };
        self.add(format!("mute.add id={} name=\"{}\" type={} flags=0", us(&blocked), esc(&name), ty));
        self.mute_insert(blocked, &name, ty, 0);
        self.add(format!("blocked_list.show {}", us(&blocked)));
        let mut killed = Vec::new();
        for n in self.live.values() {
            if matches!(n.name.as_str(), "ObjectGiveItem" | "OwnObjectGiveItem" | "UserGiveItemLegacy" | "UserGiveItem")
                && as_uuid(get(&n.payload, "from_id")) == blocked
            {
                killed.push(n.name.clone());
            }
        }
        self.add(format!("toasts.kill channel=aeed1ef0-1111-4fbc-8e36-ab00b4b3b1a3 matching=[{}]", killed.join(",")));
    }

    fn finish_postponed(&mut self, p: Postponed) {
        let name = if p.is_group { self.group_lookup_name(&p.from_id) } else { self.avatar_lookup_name(&p.from_id) };
        let mut subs = p.substitutions.clone();
        insert(&mut subs, "NAME", Value::String(name));
        let id = self.new_notification_id();
        let form = notification_form(&p.name);
        let n = Notif {
            name: p.name.clone(),
            id,
            payload: p.payload.clone(),
            substitutions: subs.clone(),
            form,
            handler: None,
            responder: p.responder,
        };
        self.live.insert(id, n);
        self.add(format!(
            "notify.postponed {} args={} payload={} from={} group={} offer_from_agent={} responder=1",
            p.name,
            sd_bin_hex(&subs),
            sd_bin_hex(&p.payload),
            us(&p.from_id),
            p.is_group as i32,
            p.offer_from_agent as i32
        ));
        self.posted.push(id);
        self.snapshot("postponed");
    }

    /// `run_idle()`.
    pub fn run_idle(&mut self) {
        while let Some(task) = self.idle.pop_front() {
            self.add("idle.run");
            match task {
                Idle::Name { id, name, is_group, then } => self.run_name_then(id, name, is_group, then),
                Idle::Postponed(p) => self.finish_postponed(p),
                Idle::RemoveObject(id) => self.add(format!("model.removeObject {}", us(&id))),
                Idle::MainCoro(m) => {
                    self.add(format!(
                        "processNewMessage offline={} dialog={} from={} group={} session={} aux={} sender={} size={} bucket={}",
                        m.offline,
                        m.dialog,
                        us(&m.from_id),
                        m.from_group as i32,
                        us(&m.session_id),
                        us(&m.aux_id),
                        m.sender.ip_and_port(),
                        m.bucket.len(),
                        hex(&m.bucket)
                    ));
                    offer::process_offline_message(self, &m);
                }
            }
        }
    }

    fn send_line(&mut self, route: &str, p: &ImPacket) {
        let (bytes, zc) = build_im(p);
        let zc = zc.map_or_else(|| "-".to_owned(), |z| hex(&z));
        self.add(format!("send {route} bytes={} zc={zc}", hex(&bytes)));
    }

    // ------------------------------------------------------------ drivers
    /// A live offer handle's state.
    pub fn offer(&self, h: u32) -> OfferInfo {
        self.offers[&h].clone()
    }

    pub fn new_offer(&mut self, o: OfferInfo) -> u32 {
        self.next_offer += 1;
        self.offers.insert(self.next_offer, o);
        self.next_offer
    }

    /// `delete o` with the log off (the harness's own deletes).
    pub fn drop_offer_silently(&mut self, h: u32) {
        self.offers.remove(&h);
    }

    /// `D().force_response(o, r)`.
    pub fn force_response(&mut self, h: u32, r: i32) {
        self.cur = Some(h);
        let mut o = self.offer(h);
        offer::force_response(self, &mut o, r);
    }

    /// `D().respond(o, notification, response)`.
    pub fn respond_offer(&mut self, h: u32, notification: &Value, response: &Value) {
        self.cur = Some(h);
        let mut o = self.offer(h);
        offer::respond(self, &mut o, notification, response);
    }

    /// `n->respond(response)` for a live notification.
    pub fn respond_notification(&mut self, id: &Uuid, response: &Value) {
        let n = self.live[id].clone();
        let live = self.live.contains_key(id);
        self.add(format!("notification.respond {} live={} selected={}", n.name, live as i32, selected(response)));
        self.snapshot("respond");
        if let Some(h) = n.responder {
            if self.offers.contains_key(&h) {
                self.respond_offer(h, &n.as_llsd(), response);
            }
        } else if let Some(handler) = n.handler {
            give::answer(self, handler, &n.as_llsd(), response);
        }
    }

    /// `respond_pending(option)`.
    pub fn respond_pending(&mut self, option: i32) {
        let Some(p) = self.pending.pop() else {
            self.add("respond: nothing pending");
            return;
        };
        let form = notification_form(&p.name);
        let mut notification = Value::Undefined;
        insert(&mut notification, "name", Value::string(&p.name));
        insert(&mut notification, "form", form.clone().unwrap_or(Value::Undefined));
        insert(&mut notification, "payload", p.payload.clone());
        let mut response = Value::Map(Default::default());
        if let Some(Value::Array(a)) = &form {
            for e in a {
                if vkstorm_inventory::llsd::coerce::as_integer(get(e, "index")) == option {
                    let k = String::from_utf8_lossy(&as_string(get(e, "name"))).into_owned();
                    insert(&mut response, &k, Value::Boolean(true));
                }
            }
        }
        self.add(format!("respond {} option={}", p.name, option));
        give::answer(self, p.handler, &notification, &response);
    }
}

pub fn perm_rec(p: &Permissions) -> String {
    format!(
        "creator={} owner={} last_owner={} group={} group_owned={} base={:08x} owner_mask={:08x} group_mask={:08x} everyone={:08x} next={:08x}",
        us(&p.creator),
        us(&p.owner),
        us(&p.last_owner),
        us(&p.group),
        p.group_owned as i32,
        p.base,
        p.owner_mask,
        p.group_mask,
        p.everyone,
        p.next
    )
}

impl Host for Fake {
    fn act(&mut self, action: Action) {
        match action {
            Action::OfferNew(o) => {
                let h = self.new_offer(o);
                self.cur = Some(h);
            }
            Action::OfferUpdate(o) => {
                if let Some(h) = self.cur {
                    self.offers.insert(h, o);
                }
            }
            Action::OfferDelete => {
                if let Some(h) = self.cur.take() {
                    self.offers.remove(&h);
                }
                self.add("responder.deleted");
            }
            Action::SendReliable { packet, route } => {
                let r = match route {
                    vkstorm_inventory::exchange::Route::Host(h) => format!("host={}", h.ip_and_port()),
                    vkstorm_inventory::exchange::Route::AgentRegion => format!("agent_region={}", self.region_host.ip_and_port()),
                };
                self.send_line(&r, &packet);
            }
            Action::PackInstantMessage(args) => {
                let bucket = args.bucket.as_ref().map_or_else(|| "EMPTY".to_owned(), |b| hex(b));
                self.add(format!("pack dialog={} to={} message=\"{}\" bucket={}", args.dialog, us(&args.to_id), esc(&args.message), bucket));
                self.packed = Some(args.packet());
            }
            Action::SendPackedToRegion => {
                let p = self.packed.take().expect("a packed message");
                let r = format!("agent_region={}", self.region_host.ip_and_port());
                self.send_line(&r, &p);
            }
            Action::Notify { name, substitutions, payload, handler } => {
                let mut line = format!("notify {name} args={} payload={}", sd_bin_hex(&substitutions), sd_bin_hex(&payload));
                let id = self.new_notification_id();
                if let Some(h) = handler {
                    line.push_str(" functor=1");
                    self.add(line);
                    let form = notification_form(&name);
                    self.live.insert(id, Notif { name: name.clone(), id, payload: payload.clone(), substitutions, form, handler: Some(h), responder: None });
                    self.pending.push(Pending { name, payload, handler: h });
                } else {
                    self.add(line);
                }
            }
            Action::NotifyPostponed { name, substitutions, payload, offer_from_agent, from_id, is_group } => {
                let p = Postponed { name, substitutions, payload, offer_from_agent, from_id, is_group, responder: self.cur };
                if self.defer {
                    self.add(format!("name.deferred postponed {}", us(&from_id)));
                    self.idle.push_back(Idle::Postponed(p));
                } else {
                    self.finish_postponed(p);
                }
            }
            Action::ForceRespond { name, response } => {
                self.new_notification_id();
                self.add(format!("notification.respond {name} live=0 selected={}", selected(&response)));
                self.snapshot("respond");
            }
            Action::AvatarNameLookup { id, then } => {
                let name = self.avatar_lookup_name(&id);
                self.add(format!("avatarname.lookup {} -> \"{}\"", us(&id), esc(&name)));
                self.name_answer(format!("avatar {}", us(&id)), id, name, false, then);
            }
            Action::GroupNameLookup { id, then } => {
                let name = self.group_lookup_name(&id);
                self.add(format!("cachename.getGroup {} -> \"{}\"", us(&id), esc(&name)));
                self.name_answer(format!("group {}", us(&id)), id, name, true, then);
            }
            Action::CacheNameGet { id, is_group, then } => {
                let name = self.group_lookup_name(&id);
                self.add(format!("cachename.get {} group={} -> \"{}\"", us(&id), is_group as i32, esc(&name)));
                self.name_answer(format!("cachename {}", us(&id)), id, name, is_group, then);
            }
            Action::ObserverDone(o) => {
                let obs = self.observers[o as usize].clone();
                match &obs.kind {
                    ObserverKind::FetchItems { .. } => self.add("fetch.done LLInventoryFetchItemsObserver"),
                    ObserverKind::OpenAgentOffer { from_name, manual, .. } => {
                        self.add(format!("open_inventory_offer items={} from=\"{}\" manual={}", ids(&obs.complete), esc(from_name), *manual as i32));
                        self.add("model.removeObserver LLOpenAgentOffer");
                    }
                    ObserverKind::DiscardAgentOffer { object, .. } => {
                        self.add("idle.queue");
                        self.idle.push_back(Idle::RemoveObject(*object));
                        self.add("model.removeObserver LLDiscardAgentOffer");
                    }
                    ObserverKind::RlvGiveToRlvAgentOffer { .. } => self.add("RlvGiveToRLVAgentOffer.done"),
                    _ => {}
                }
            }
            Action::ObserverAdd(o) => {
                let c = self.observers[o as usize].class();
                self.add(format!("model.addObserver {c}"));
            }
            Action::ObserverDrop(_) => {}
            Action::RecentPeopleAdd(id) => self.add(format!("recent_people.add {}", us(&id))),
            Action::FirstUseNewInventory => self.add("first_use.newInventory"),
            Action::NearbyChat(m) => self.add(format!("nearby_chat \"{}\"", esc(&m))),
            Action::UiSound(n) => self.add(format!("ui_sound {n}")),
            Action::InventoryPanelSelect { auto_open, id } => self.add(format!("inventory_panel.select auto_open={} id={}", auto_open as i32, us(&id))),
            Action::GroupPanelShowNotice { subject, message, group_id, has_inventory, inventory_name, with_offer } => {
                self.add(format!(
                    "group_panel.showNotice group={} has_inventory={} inventory_name=\"{}\" offer={} subject=\"{}\" message=\"{}\"",
                    us(&group_id),
                    has_inventory as i32,
                    esc(&inventory_name),
                    with_offer as i32,
                    esc(&subject),
                    esc(&message)
                ));
                let h = if with_offer { self.cur } else { None };
                self.group_panel_offers.insert(group_id, h);
                self.snapshot("group_panel");
            }
            Action::RlvSendNotification(t) => self.add(format!("rlv.sendNotification \"{}\"", esc(&t))),
            Action::RlvNotifyBlockedShare { args } => self.add(format!("rlv.notifyBlocked blocked_share args={}", sd_bin_hex(&args))),
            Action::PostToMainCoro(m) => {
                self.add("main_coro.post");
                self.idle.push_back(Idle::MainCoro(m));
            }
            Action::HudBeam { to } => {
                self.add("hud.createViewerEffect type=9 send_to_sim=1");
                self.add("hud.beam source=self");
                let t = if self.objects.contains_key(&to) { us(&to) } else { "null".into() };
                self.add(format!("hud.beam target={t}"));
                self.add("hud.beam duration=1");
                self.add("hud.beam color");
            }
            Action::FloaterToolsDirty => self.add("floater_tools.dirty"),
            Action::MuteAutoRemove { id, reason } => {
                if self.muted(&id, b"", 0) {
                    self.add(format!("mute.remove id={} type=1", us(&id)));
                    self.mutes.remove(&id);
                    match self.avatar_names.get(&id).cloned() {
                        Some(n) => self.add(format!("mute.auto_unmute_notice id={} name=\"{}\" reason={}", us(&id), esc(&n), reason)),
                        None => {
                            let name = self.avatar_lookup_name(&id);
                            self.add(format!("avatarname.lookup {} -> \"{}\"", us(&id), esc(&name)));
                            self.name_answer(format!("avatar {}", us(&id)), id, name, false, NameThen::AutoUnmuteNotice { reason });
                        }
                    }
                }
            }
            Action::ImSystemMessage { session, name, args } => {
                self.add(format!("im.system_message session={} {} args={}", us(&session), name, sd_bin_hex(&args)))
            }
            Action::ImLogToFile { file, from, id, template, args } => {
                let mut line = template.clone();
                line.push(b' ');
                line.extend_from_slice(sd_notation(&args).as_bytes());
                self.add(format!("im.log_to_file file=\"{}\" from=\"{}\" id={} line=\"{}\"", esc(&file), esc(&from), us(&id), esc(&line)));
            }
            Action::DeleteObject(id) => {
                self.add(format!("model.deleteObject {}", us(&id)));
                if let Some(it) = self.items.remove(&id) {
                    if let Some(v) = self.item_tree.get_mut(&it.rec.parent) {
                        v.retain(|x| *x != id);
                    }
                }
            }
            Action::NotifyObservers => self.add("model.notifyObservers"),
            Action::FloaterHide(n) => self.add(format!("floater.hide {n}")),
            Action::WarnMoveInventory(mv) => self.add(format!("warn_move_inventory {}", move_list(&mv))),
            Action::MoveTaskInventory { notification, response, mv } => {
                let option = selected_option(&notification, &response);
                self.add(format!("move_task_inventory_callback option={option} {}", move_list(&mv)));
            }
        }
    }

    fn agent_id(&self) -> Uuid { self.agent }
    fn agent_session_id(&self) -> Uuid { self.session }
    fn agent_group_id(&self) -> Uuid { self.agent_group }
    fn agent_position(&self) -> [f32; 3] { self.position }
    fn agent_region_host(&self) -> NetHost { self.region_host }
    fn is_do_not_disturb(&self) -> bool { self.dnd }
    fn have_avatar(&self) -> bool { self.have_avatar }
    fn avatar_valid(&self) -> bool { self.have_avatar && self.avatar_valid }
    fn avatar_fullname(&self) -> Vec<u8> { self.fullname.clone() }
    fn is_wearing_attachment(&self, id: &Uuid) -> bool { self.attachments.contains(id) }
    fn is_wearing_item(&self, id: &Uuid) -> bool { self.wearables.contains(id) }
    fn is_gesture_active(&self, id: &Uuid) -> bool { self.gestures.contains(id) }
    fn avatar_sitting(&self) -> bool { self.sitting }
    fn avatar_root(&self) -> Option<Uuid> { self.avatar_root }
    fn setting_bool(&self, name: &str) -> bool {
        *self.settings_b.get(name).unwrap_or_else(|| panic!("setting {name} is not pinned"))
    }
    fn setting_u32(&self, name: &str) -> u32 {
        *self.settings_u.get(name).unwrap_or_else(|| panic!("setting {name} is not pinned"))
    }
    fn autoresponse_text(&self, _from: &Uuid) -> Vec<u8> { self.dnd_response.clone() }
    fn trans(&self, key: &str, args: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut out = format!("{{{key}").into_bytes();
        let mut sorted: Vec<&(&str, Vec<u8>)> = args.iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(b.0));
        for (k, v) in sorted {
            out.push(b'|');
            out.extend_from_slice(k.as_bytes());
            out.push(b'=');
            out.extend_from_slice(v);
        }
        out.push(b'}');
        out
    }
    fn slurl_location(&self, url: &[u8]) -> Vec<u8> {
        [b"location(".as_slice(), url, b")"].concat()
    }
    fn rlv_anonym(&self, name: &[u8]) -> Vec<u8> {
        [b"anon(".as_slice(), name, b")"].concat()
    }
    fn rlv_anonym_avatar(&self, id: &Uuid) -> Option<Vec<u8>> {
        self.avatar_names.get(id).map(|n| self.rlv_anonym(n))
    }
    fn slurl(&self, command: &str, id: &Uuid, verb: &[u8]) -> Vec<u8> {
        [format!("secondlife:///app/{command}/{}/", us(id)).as_bytes(), verb].concat()
    }
    fn trans_app_title(&self) -> Vec<u8> { b"Vulkanstorm".to_vec() }
    fn trim_resident_surname(&self) -> bool { self.trim_resident }
    fn rlv_enabled(&self) -> bool { self.rlv_enabled }
    fn rlv_can_show_name(&self, _id: &Uuid) -> bool { self.rlv_can_show_names }
    fn rlv_can_give_inventory(&self, id: &Uuid) -> bool { !self.rlv_share_blocked.contains(id) }
    fn rlv_is_nearby(&self, id: &Uuid) -> bool { self.rlv_nearby.contains(id) }
    fn rlv_has_open_im(&self, _id: &Uuid) -> bool { false }
    fn rlv_has_open_profile(&self, _id: &Uuid) -> bool { false }
    fn rlv_forbid_give_to_rlv(&self) -> bool { self.rlv_forbid }
    fn rlv_has_shared_root(&self) -> bool { self.rlv_shared_root != NULL_ID }
    fn rlv_shared_root_id(&self) -> Uuid { self.rlv_shared_root }
    fn rlv_locked_attachment(&self, _root: &Uuid) -> bool { self.rlv_locked_attachment }
    fn rlv_has_behaviour(&self, b: RlvBehaviour) -> bool {
        match b {
            RlvBehaviour::Unsit => self.rlv_unsit,
            RlvBehaviour::SitTp => self.rlv_sittp,
        }
    }
    fn is_muted(&self, id: &Uuid, name: &[u8], flags: u32) -> bool { self.muted(id, name, flags) }
    fn avatar_user_name(&self, id: &Uuid) -> Option<Vec<u8>> { self.avatar_names.get(id).cloned() }
    fn avatar_complete_name(&self, id: &Uuid) -> Option<Vec<u8>> { self.avatar_names.get(id).cloned() }
    fn group_name(&self, id: &Uuid) -> Option<Vec<u8>> { self.group_names.get(id).cloned() }
    fn find_id_by_name(&self, name: &[u8]) -> Uuid { self.ids_by_legacy_name.get(name).copied().unwrap_or(NULL_ID) }
    fn notification_live(&self, id: &Uuid) -> bool { self.live.contains_key(id) }
    fn notification_form(&self, name: &str) -> Option<Value> { notification_form(name) }
    fn item(&self, id: &Uuid) -> Option<ModelItem> { self.items.get(id).map(|i| i.rec.clone()) }
    fn category(&self, id: &Uuid) -> Option<ModelCat> {
        if self.cat_map.contains(id) { self.cat_records.get(id).cloned() } else { None }
    }
    fn children(&self, parent: &Uuid) -> (Vec<ModelCat>, Vec<ModelItem>) {
        let cats = self.cat_tree.get(parent).map_or_else(Vec::new, |v| v.iter().map(|c| self.cat_records[c].clone()).collect());
        let items = self.item_tree.get(parent).map_or_else(Vec::new, |v| v.iter().map(|i| self.items[i].rec.clone()).collect());
        (cats, items)
    }
    fn is_category_complete(&self, id: &Uuid) -> bool { !self.incomplete.contains(id) }
    fn find_category_for_type(&mut self, ty: FolderType) -> Uuid {
        let id = self.system_folders.get(&ty.0).copied().unwrap_or_else(|| folder_for(ty.0));
        self.add(format!("model.findCategoryUUIDForType {} -> {}", ty.0, us(&id)));
        id
    }
    fn generate_uuid(&mut self) -> Uuid {
        self.generate_counter += 1;
        let id = uid(0x7a00_0000, self.generate_counter);
        self.add(format!("uuid.generate {}", us(&id)));
        id
    }
    fn now(&self) -> f64 { 1_700_000_000.0 }
    fn frame_time(&self) -> f64 { self.clock }
    fn startup_started(&self) -> bool { self.startup_started }
    fn throttle(&mut self) -> &mut Throttle { &mut self.throttle }
    fn im_session_name(&self, s: &Uuid) -> Option<Vec<u8>> {
        self.im_sessions.contains(s).then(|| format!("session {}", short(s)).into_bytes())
    }
    fn max_inventory_items_transfer(&self) -> i32 { self.max_transfer }
    fn object(&self, id: &Uuid) -> Option<ViewerObject> { self.objects.get(id).map(|o| o.obj) }
    fn object_inventory(&self, id: &Uuid) -> Vec<TaskObject> { self.objects.get(id).map_or_else(Vec::new, |o| o.inventory.clone()) }
    fn have_floater_tools(&self) -> bool { true }
    fn observer_new(&mut self, kind: ObserverKind) -> ObserverId {
        match &kind {
            ObserverKind::RlvGiveToRlvAgentOffer { folder } => self.add(format!("new RlvGiveToRLVAgentOffer {}", us(folder))),
            ObserverKind::RlvGiveToRlvTaskOffer { transaction } => self.add(format!("new RlvGiveToRLVTaskOffer {}", us(transaction))),
            ObserverKind::OpenTaskGroupOffer => self.add("new LLOpenTaskGroupOffer"),
            _ => {}
        }
        self.observers.push(Obs { kind, complete: Vec::new(), incomplete: Vec::new() });
        (self.observers.len() - 1) as ObserverId
    }
    fn observer_start(&mut self, o: ObserverId) -> bool {
        let mut obs = self.observers[o as usize].clone();
        let ids_list: Vec<Uuid> = match &obs.kind {
            ObserverKind::FetchItems { id } | ObserverKind::OpenAgentOffer { id, .. } => vec![*id],
            ObserverKind::DiscardAgentOffer { object, .. } => vec![*object],
            ObserverKind::RlvGiveToRlvAgentOffer { folder } => vec![*folder],
            _ => return true,
        };
        if matches!(obs.kind, ObserverKind::OpenAgentOffer { .. }) {
            for id in &ids_list {
                if self.category(id).is_some() {
                    obs.complete.push(*id);
                }
            }
        }
        for id in &ids_list {
            if self.items.get(id).is_some_and(|i| i.rec.finished) {
                obs.complete.push(*id);
                continue;
            }
            if self.category(id).is_some() || *id == NULL_ID {
                continue;
            }
            obs.incomplete.push(*id);
        }
        self.add(format!("fetch.start {} ids={} complete={} incomplete={}", obs.class(), ids(&ids_list), ids(&obs.complete), ids(&obs.incomplete)));
        let finished = obs.incomplete.is_empty();
        self.observers[o as usize] = obs;
        finished
    }
}

/// The common agent (`setup()`): fixed ids, position, name, region host,
/// every setting pinned to its default, the other avatar's and the group's
/// names cached.
pub fn setup() -> Fake {
    let mut f = Fake {
        agent: agent(),
        session: session(),
        position: [128.25, 64.5, 22.75],
        region_host: NetHost::parse(AGENT_HOST),
        fullname: b"Oracle Tester".to_vec(),
        ..Fake::default()
    };
    for (k, v) in [
        ("AutoAcceptNewInventory", false),
        ("FSUseLegacyInventoryAcceptMessages", false),
        ("FSLogAutoAcceptInventoryToChat", true),
        ("FSShowAutoAcceptInventoryInNotifications", true),
        ("ShowNewInventory", true),
        ("ShowInInventory", false),
        ("FSDismissGroupNoticeAttachmentsToTrash", false),
        ("LogInventoryDecline", true),
        ("FSCreateGiveInventoryParticleEffect", true),
        ("UseLegacyIMLogNames", true),
        ("FSNotifyIncomingObjectSpamFrom", true),
        ("FSNotifyIncomingObjectSpam", true),
    ] {
        f.settings_b.insert(k.to_owned(), v);
    }
    f.settings_u.insert("FSOfferThrottleMaxCount".to_owned(), 5);
    f.dnd_response = b"This resident has turned on 'Do Not Disturb' and will see your message later.".to_vec();
    f.avatar_names.insert(other(), b"other.sender".to_vec());
    f.group_names.insert(group(), b"Oracle Group".to_vec());
    f
}

pub mod incoming;

// ============================================================ case output

/// The output of a corpus: `<case>\t<key>\t<value>` lines (no notes).
#[derive(Default)]
pub struct Out {
    pub lines: Vec<String>,
}

impl Out {
    pub fn put(&mut self, c: &str, k: &str, v: &str) {
        self.lines.push(format!("{c}\t{k}\t{v}"));
    }

    /// `put_events(s, c)`: the event log as `eNN` lines.
    pub fn events(&mut self, f: &mut Fake, c: &str) {
        for (n, e) in f.ev.drain(..).enumerate() {
            self.lines.push(format!("{c}\te{n:02}\t{e}"));
        }
    }
}

/// `put_world(s, c)`: the agent, worn state, limits, settings, RLVa, mutes,
/// IM sessions and names, then the model.
pub fn put_world(out: &mut Out, f: &Fake, c: &str) {
    let mut n = 0;
    let mut line = |out: &mut Out, v: String| {
        out.put(c, &format!("input.{n:02}"), &v);
        n += 1;
    };
    line(
        out,
        format!(
            "agent id={} session={} avatar_valid={} fullname=\"{}\" sitting={} dnd={} groups={}",
            us(&f.agent),
            us(&f.session),
            f.avatar_valid as i32,
            esc(&f.fullname),
            f.sitting as i32,
            f.dnd as i32,
            id_set(&f.groups)
        ),
    );
    line(out, format!("worn attachments={} wearables={} active_gestures={}", id_set(&f.attachments), id_set(&f.wearables), id_set(&f.gestures)));
    let mut folders = String::new();
    for (k, v) in &f.system_folders {
        let _ = write!(folders, " {k}={}", us(v));
    }
    line(
        out,
        format!(
            "limits max_inventory_items_transfer={} system_folder_overrides=[{folders} ] (other folder types: 000000f0-0000-4000-8000-<type as U16>)",
            f.max_transfer
        ),
    );
    let mut set = String::new();
    for (k, v) in &f.settings_b {
        let _ = write!(set, " {k}={}", *v as i32);
    }
    for (k, v) in &f.settings_u {
        let _ = write!(set, " {k}={v}");
    }
    line(out, format!("settings{set}"));
    line(
        out,
        format!(
            "rlv enabled={} forbid_give_to_rlv={} shared_root={} can_show_names={} locked_attachment={} unsit={} sittp={} share_blocked={} nearby={}",
            f.rlv_enabled as i32,
            f.rlv_forbid as i32,
            us(&f.rlv_shared_root),
            f.rlv_can_show_names as i32,
            f.rlv_locked_attachment as i32,
            f.rlv_unsit as i32,
            f.rlv_sittp as i32,
            id_set(&f.rlv_share_blocked),
            id_set(&f.rlv_nearby)
        ),
    );
    let mut mutes = String::new();
    for m in f.mutes.values() {
        let _ = write!(mutes, " {}:{}:{}:\"{}\"", us(&m.id), m.ty, m.flags, esc(&m.name));
    }
    let mut legacy = String::new();
    for l in &f.legacy_mutes {
        let _ = write!(legacy, " \"{}\"", esc(l));
    }
    line(out, format!("mutes [{mutes} ] legacy [{legacy} ]"));
    let mut names = String::new();
    for (k, v) in &f.avatar_names {
        let _ = write!(names, " {}=\"{}\"", us(k), esc(v));
    }
    line(out, format!("im_sessions={} avatar_names [{names} ]", id_set(&f.im_sessions)));
    // the model: parents in id order, categories then items
    let mut parents: BTreeSet<Uuid> = f.cat_tree.keys().copied().collect();
    parents.extend(f.item_tree.keys().copied());
    for p in parents {
        if let Some(cs) = f.cat_tree.get(&p) {
            for c in cs {
                line(out, f.cat_rec_line(&f.cat_records[c]));
            }
        }
        if let Some(is) = f.item_tree.get(&p) {
            for i in is {
                line(out, f.item_rec_line(&f.items[i]));
            }
        }
    }
}

/// `put_items(s, c, items)`.
pub fn put_items(out: &mut Out, f: &Fake, c: &str, items: &[&Item]) {
    let mut n = 0;
    for it in items {
        out.put(c, &format!("input.{n:02}"), &f.item_rec_line(it));
        n += 1;
    }
    out.put(
        c,
        &format!("input.{n:02}"),
        &format!(
            "avatar_valid={} attachments={} wearables={} active_gestures={} groups={}",
            f.avatar_valid as i32,
            id_set(&f.attachments),
            id_set(&f.wearables),
            id_set(&f.gestures),
            id_set(&f.groups)
        ),
    );
}

/// The filter of `move_inv_category_world_to_agent` (fake: passes the ids in
/// `passes`).
pub struct PassFilter {
    pub types: u64,
    pub passes: BTreeSet<Uuid>,
}

impl ItemFilter for PassFilter {
    fn filter_types(&self) -> u64 {
        self.types
    }
    fn check(&mut self, item: &TaskObject) -> bool {
        self.passes.contains(&item.id)
    }
}

// =============================================================== fixtures

pub fn fixture_path(area: &str, name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(area).join(name)
}

/// The data lines of a fixture (no `#` header lines).
pub fn read_lines(area: &str, name: &str) -> Vec<String> {
    let path = fixture_path(area, name);
    let data = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let text = String::from_utf8(data).expect("fixture lines are escaped ASCII");
    let mut v: Vec<String> = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l).to_owned()).filter(|l| !l.starts_with('#')).collect();
    if v.last().is_some_and(|l| l.is_empty()) {
        v.pop();
    }
    v
}

pub fn is_note(line: &str) -> bool {
    line.split('\t').nth(1).is_some_and(|k| k.starts_with("note."))
}

/// The compared lines of a fixture (notes dropped).
pub fn compared(area: &str, name: &str) -> Vec<String> {
    read_lines(area, name).into_iter().filter(|l| !is_note(l)).collect()
}

fn case_of(l: &str) -> &str {
    l.split('\t').next().unwrap_or("")
}

/// Compares two line lists case by case (aligned by case id) and panics with
/// the first differences.
pub fn assert_lines(label: &str, expected: &[String], actual: &[String]) {
    if expected == actual {
        let cases: HashSet<&str> = expected.iter().map(|l| case_of(l)).collect();
        println!("{label}: {} lines, {} cases identical", expected.len(), cases.len());
        return;
    }
    let group = |v: &[String]| {
        let mut order = Vec::new();
        let mut m: HashMap<String, Vec<String>> = HashMap::new();
        for l in v {
            let c = case_of(l).to_owned();
            if !m.contains_key(&c) {
                order.push(c.clone());
            }
            m.entry(c).or_default().push(l.clone());
        }
        (order, m)
    };
    let (eo, em) = group(expected);
    let (ao, am) = group(actual);
    let mut report = String::new();
    let mut bad = 0;
    for c in &eo {
        let e = &em[c];
        match am.get(c) {
            None => {
                bad += 1;
                if bad <= 5 {
                    let _ = writeln!(report, "case {c}: missing");
                }
            }
            Some(a) if a != e => {
                bad += 1;
                if bad <= 5 {
                    let i = e.iter().zip(a.iter()).position(|(x, y)| x != y).unwrap_or(e.len().min(a.len()));
                    let clip = |s: Option<&String>| s.map_or("<none>".to_owned(), |s| s.chars().take(600).collect());
                    let _ = writeln!(report, "case {c}: line {i}\n  expected {}\n  actual   {}", clip(e.get(i)), clip(a.get(i)));
                }
            }
            _ => {}
        }
    }
    for c in &ao {
        if !em.contains_key(c) {
            bad += 1;
            if bad <= 5 {
                let _ = writeln!(report, "case {c}: unexpected");
            }
        }
    }
    if bad == 0 && eo != ao {
        let _ = writeln!(report, "case order differs");
        bad = 1;
    }
    panic!("{label}: {bad} differing cases\n{report}");
}

// ================================================================ digests

pub fn fnv1a(s: &str) -> u32 {
    let mut h: u32 = 2166136261;
    for b in s.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(16777619);
    }
    h
}

pub fn in_sample(case: &str, modulus: u32) -> bool {
    fnv1a(case) % modulus == 0
}

/// SHA-256 (the recorder's).
#[derive(Clone)]
pub struct Sha256 {
    h: [u32; 8],
    block: [u8; 64],
    used: usize,
    total: u64,
}

const SHA_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be,
    0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa,
    0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85,
    0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
    0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f,
    0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

impl Default for Sha256 {
    fn default() -> Self {
        Sha256 {
            h: [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19],
            block: [0; 64],
            used: 0,
            total: 0,
        }
    }
}

impl Sha256 {
    fn compress(h: &mut [u32; 8], p: &[u8]) {
        let mut w = [0u32; 64];
        for (i, word) in p.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = *h;
        for (k, wi) in SHA_K.iter().zip(w.iter()) {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(*k).wrapping_add(*wi);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (x, v) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(v);
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.total += data.len() as u64;
        if self.used > 0 {
            let take = data.len().min(64 - self.used);
            self.block[self.used..self.used + take].copy_from_slice(&data[..take]);
            self.used += take;
            data = &data[take..];
            if self.used < 64 {
                return;
            }
            let block = self.block;
            Self::compress(&mut self.h, &block);
            self.used = 0;
        }
        let mut blocks = data.chunks_exact(64);
        for b in &mut blocks {
            Self::compress(&mut self.h, b);
        }
        let rem = blocks.remainder();
        self.block[..rem.len()].copy_from_slice(rem);
        self.used = rem.len();
    }

    pub fn hex(mut self) -> String {
        let bits = self.total.wrapping_mul(8);
        self.update(&[0x80]);
        while self.used != 56 {
            self.update(&[0]);
        }
        self.update(&bits.to_be_bytes());
        self.h.iter().map(|w| format!("{w:08x}")).collect()
    }
}

/// The recorder's `CollectSink`: the stream's digest, chunk digests, sample
/// and outcome classes.
pub struct Collect {
    pub modulus: u32,
    pub chunk: usize,
    pub strat: bool,
    sha: Sha256,
    pub lines: usize,
    pub cases: usize,
    pub sample: Vec<String>,
    cur: String,
    cur_lines: Vec<String>,
    cur_outcome: String,
    chunk_sha: Sha256,
    chunk_n: usize,
    chunk_first: String,
    chunk_last: String,
    pub chunk_lines: Vec<String>,
    pub classes: HashMap<String, (usize, String)>,
    pub class_order: Vec<String>,
    rlv_seen: HashSet<(String, String)>,
}

fn rlv_state(case: &str) -> Option<String> {
    let a = case.find(".rlv1.")?;
    let rest = &case[a..];
    Some(match rest.find(".type_") {
        Some(b) => rest[..b].to_owned(),
        None => rest.to_owned(),
    })
}

impl Collect {
    pub fn new(modulus: u32, chunk: usize, strat: bool) -> Collect {
        Collect {
            modulus,
            chunk,
            strat,
            sha: Sha256::default(),
            lines: 0,
            cases: 0,
            sample: Vec::new(),
            cur: String::new(),
            cur_lines: Vec::new(),
            cur_outcome: String::new(),
            chunk_sha: Sha256::default(),
            chunk_n: 0,
            chunk_first: String::new(),
            chunk_last: String::new(),
            chunk_lines: Vec::new(),
            classes: HashMap::new(),
            class_order: Vec::new(),
            rlv_seen: HashSet::new(),
        }
    }

    fn close_chunk(&mut self) {
        if self.chunk_n == 0 {
            return;
        }
        let sha = std::mem::take(&mut self.chunk_sha).hex();
        self.chunk_lines.push(format!(
            "chunk={} first={} last={} cases={} sha256={sha}",
            self.chunk_lines.len(),
            self.chunk_first,
            self.chunk_last,
            self.chunk_n
        ));
        self.chunk_n = 0;
    }

    fn finish_case(&mut self) {
        if self.cur.is_empty() {
            return;
        }
        let outcome = std::mem::take(&mut self.cur_outcome);
        let entry = self.classes.entry(outcome.clone()).or_insert((0, String::new()));
        entry.0 += 1;
        if entry.0 == 1 {
            entry.1 = self.cur.clone();
            self.class_order.push(outcome.clone());
        }
        let mut take = self.modulus != 0 && in_sample(&self.cur, self.modulus);
        if self.strat && !take {
            if let Some(rs) = rlv_state(&self.cur) {
                let rlv_action = outcome.contains("RlvGiveToRLV") || outcome.contains("rlv.");
                if rlv_action && self.rlv_seen.insert((outcome.clone(), rs)) {
                    take = true;
                }
            }
        }
        let lines = std::mem::take(&mut self.cur_lines);
        if take {
            self.sample.extend(lines);
        }
        if self.chunk_n == 0 {
            self.chunk_first = self.cur.clone();
        }
        self.chunk_last = self.cur.clone();
        self.chunk_n += 1;
        if self.chunk_n == self.chunk {
            self.close_chunk();
        }
        self.cur.clear();
    }

    pub fn line(&mut self, l: &str) {
        let c = case_of(l);
        if c != self.cur {
            self.finish_case();
            self.cur = c.to_owned();
            self.cases += 1;
        }
        self.cur_lines.push(l.to_owned());
        self.sha.update(l.as_bytes());
        self.sha.update(b"\n");
        self.chunk_sha.update(l.as_bytes());
        self.chunk_sha.update(b"\n");
        self.lines += 1;
        self.cur_outcome.push_str(&l[l.find('\t').unwrap() + 1..]);
        self.cur_outcome.push('\n');
    }

    pub fn lines_from(&mut self, v: Vec<String>) {
        for l in v {
            self.line(&l);
        }
    }

    pub fn finish(&mut self) -> String {
        self.finish_case();
        self.close_chunk();
        std::mem::take(&mut self.sha).hex()
    }

    /// The outcome classes as `outcomes.txt` writes them (no header).
    pub fn outcomes_text(&self, hashed: bool) -> Vec<String> {
        let mut out = Vec::new();
        for (n, key) in self.class_order.iter().enumerate() {
            let (count, first) = &self.classes[key];
            if hashed {
                let mut h = Sha256::default();
                h.update(key.as_bytes());
                out.push(format!("class {n} count {count} first {first} sha256={}", h.hex()));
            } else {
                out.push(format!("class {n} count {count} first {first}"));
                for l in key.lines() {
                    out.push(l.to_owned());
                }
            }
        }
        out
    }
}

/// Checks a sweep against its `.digest` (whole stream, chunks), `.sample.txt`
/// and, for the policy tables, `.outcomes.txt`.
pub fn check_sweep(area: &str, name: &str, mut c: Collect, outcomes: Option<bool>) {
    let sha = c.finish();
    let digest = read_lines(area, &format!("{name}.digest"));
    let field = |k: &str| digest.iter().find_map(|l| l.strip_prefix(&format!("{k}="))).unwrap_or_else(|| panic!("{name}.digest: no {k}")).to_owned();
    let exp_chunks: Vec<&String> = digest.iter().filter(|l| l.starts_with("chunk=")).collect();
    let mut failures = String::new();
    if field("lines") != c.lines.to_string() || field("cases") != c.cases.to_string() || field("sha256") != sha {
        let _ = writeln!(failures, "{area}/{name}: stream expected lines={} cases={} sha256={} actual lines={} cases={} sha256={sha}", field("lines"), field("cases"), field("sha256"), c.lines, c.cases);
    }
    for i in 0..exp_chunks.len().max(c.chunk_lines.len()) {
        let e = exp_chunks.get(i).map_or("<none>", |s| s.as_str());
        let a = c.chunk_lines.get(i).map_or("<none>", |s| s.as_str());
        if e != a {
            let _ = writeln!(failures, "  first changed chunk: expected {e}\n                       actual   {a}");
            break;
        }
    }
    let exp_sample = compared(area, &format!("{name}.sample.txt"));
    let act_sample: Vec<String> = c.sample.iter().filter(|l| !is_note(l)).cloned().collect();
    if exp_sample != act_sample {
        let r = std::panic::catch_unwind(|| assert_lines(&format!("{area}/{name} sample"), &exp_sample, &act_sample));
        if let Err(e) = r {
            let msg = e.downcast_ref::<String>().cloned().unwrap_or_default();
            let _ = writeln!(failures, "{msg}");
        }
    }
    if let Some(hashed) = outcomes {
        let exp = read_lines(area, &format!("{name}.outcomes.txt"));
        let act = c.outcomes_text(hashed);
        if exp != act {
            let i = exp.iter().zip(act.iter()).position(|(x, y)| x != y).unwrap_or(exp.len().min(act.len()));
            let _ = writeln!(
                failures,
                "{area}/{name} outcome classes: {} expected lines, {} actual; first difference at line {i}:\n  expected {}\n  actual   {}",
                exp.len(),
                act.len(),
                exp.get(i).map_or("<none>", |s| s.as_str()),
                act.get(i).map_or("<none>", |s| s.as_str())
            );
        }
    }
    assert!(failures.is_empty(), "{failures}");
    println!(
        "{area}/{name}: {} cases, {} lines, digest and {} chunks identical; sample {} lines{}",
        c.cases,
        c.lines,
        c.chunk_lines.len(),
        exp_sample.len(),
        if outcomes.is_some() { format!("; {} outcome classes identical", c.class_order.len()) } else { String::new() }
    );
}

// =================================================================== rng

/// xorshift64* (`Rng`).
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(if seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { seed })
    }
    pub fn next(&mut self) -> u64 {
        let mut s = self.0;
        s ^= s >> 12;
        s ^= s << 25;
        s ^= s >> 27;
        self.0 = s;
        s.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn below(&mut self, n: u32) -> u32 {
        (self.next() % n as u64) as u32
    }
    pub fn coin(&mut self) -> bool {
        self.next() & 1 == 1
    }
}
