//! Rung seven, sub-rung R4b: inventory offers and giving via
//! `vkstorm-inventory`'s `exchange` module (`vks_inv_r4b_*`).
//!
//! Rust decides; the viewer keeps its state and does the work. Each call
//! wraps the caller's [`VksInvR4bHost`] (a context pointer, a query callback
//! and an act callback) in an adapter that implements
//! [`vkstorm_inventory::exchange::Host`]: every host query is one
//! `VKS_INV_R4B_Q_*` query whose answer comes back through the
//! `vks_inv_r4b_out_*` sink, and every [`Action`] is one flat
//! [`VksInvR4bAction`] handed to `act` in order, its strings and binary
//! LLSD borrowed from buffers that live until `act` returns. An action may
//! run C++ that calls back into these functions; each call has its own
//! adapter, so the calls are re-entrant.
//!
//! The offer a call works on is the C++ `LLOfferInfo`: its fields come in as
//! a [`VksInvOffer`], and its changes go out as `OFFER_NEW`, `OFFER_UPDATE`
//! and `OFFER_DELETE` actions at the point the C++ made them.
//!
//! LLSD crosses as binary LLSD without a header, an empty buffer for
//! undefined. The throttle state is the caller's [`VksInvR4bThrottle`]: read
//! whenever the decision asks for it and written back before the next read
//! and when the call ends.
//!
//! Every call catches panics (false, `ACCEPT_NO`, null or nothing, with the
//! text in `vks_inv_last_error`).

use std::ptr;

use vkstorm_inventory::exchange::give::{self, Cargo, DragSource, DragState, Functor, ItemFilter};
use vkstorm_inventory::exchange::host::{
    Action, Handler, Host, ImPacket, LogLevel, ModelCat, ModelItem, MoveInv, NameThen, ObserverId,
    ObserverKind, OfflineMessage, PackArgs, RlvBehaviour, Route, TaskObject, ViewerObject,
};
use vkstorm_inventory::exchange::im::Acceptance;
use vkstorm_inventory::exchange::offer::{self, ImArgs};
use vkstorm_inventory::exchange::throttle::{self, Throttle};
use vkstorm_inventory::exchange::{NetHost, OfferInfo};
use vkstorm_inventory::perms::{Permissions, Uuid};
use vkstorm_inventory::types::{AssetType, FolderType};
use vkstorm_llsd::{Format, Limits, Options, Value};

use crate::inventory::{VksInvBytes, bytes, guard, set_error, uuid_arg};

// ---------------------------------------------------------------------------
// Records

/// `LLHost`: the address as `mIP` lies in memory, and the port.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VksInvNetHost {
    pub ip: [u8; 4],
    pub port: u32,
}

/// `LLOfferInfo`'s fields.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvOffer {
    pub im: i32,
    pub from_id: Uuid,
    pub from_group: bool,
    pub from_object: bool,
    pub transaction_id: Uuid,
    pub folder_id: Uuid,
    pub object_id: Uuid,
    pub ty: i32,
    pub from_name: VksInvBytes,
    pub desc: VksInvBytes,
    pub host: VksInvNetHost,
    pub persist: bool,
}

/// An item as the model holds it.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvR4bItem {
    pub id: Uuid,
    pub parent: Uuid,
    pub ty: i32,
    pub asset: Uuid,
    pub name: VksInvBytes,
    pub perms: Permissions,
    pub finished: bool,
}

/// A category as the model holds it.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvR4bCat {
    pub id: Uuid,
    pub parent: Uuid,
    pub preferred: i32,
    pub name: VksInvBytes,
}

/// `LLViewerObject` as the drop tests see it.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvR4bViewerObject {
    pub id: Uuid,
    pub perm_modify: bool,
    pub flag_allow_inventory_add: bool,
    pub perm_you_owner: bool,
    pub pcode: u8,
    pub is_attachment: bool,
    pub root: Uuid,
}

/// One entry of an object's inventory.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvR4bTaskObject {
    pub id: Uuid,
    pub ty: i32,
    pub has_perms: bool,
    pub perms: Permissions,
}

/// An instant message (see the header for the three uses).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvR4bIm {
    pub agent_id: Uuid,
    pub session_id: Uuid,
    pub from_group: bool,
    pub to_agent_id: Uuid,
    pub parent_estate_id: u32,
    pub region_id: Uuid,
    pub position: [f32; 3],
    pub offline: u8,
    pub dialog: i32,
    pub id: Uuid,
    pub timestamp: u32,
    pub from_agent_name: VksInvBytes,
    pub message: VksInvBytes,
    pub bucket: VksInvBytes,
    pub has_bucket: bool,
}

/// `LLMoveInv`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvR4bMove {
    pub object_id: Uuid,
    pub category_id: Uuid,
    pub pairs: *const u8,
    pub count: usize,
}

/// A host query.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvR4bQuery {
    pub code: u32,
    pub id: *const u8,
    pub id2: *const u8,
    pub text: VksInvBytes,
    pub text2: VksInvBytes,
    pub number: i32,
    pub flag: bool,
    pub list: *const VksInvBytes,
    pub list_len: usize,
}

/// An action.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvR4bAction {
    pub kind: u32,
    pub number: [i32; 3],
    pub flag: [bool; 2],
    pub id: [Uuid; 2],
    pub text: [VksInvBytes; 4],
    pub sd: [VksInvBytes; 2],
    pub offer: *const VksInvOffer,
    pub im: *const VksInvR4bIm,
    pub host: VksInvNetHost,
    pub mv: *const VksInvR4bMove,
}

/// The offer throttle's state, owned by the caller.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvR4bThrottle {
    pub expiry: f64,
    pub count: u32,
    pub logged: bool,
}

/// The caller's host.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvR4bHost {
    pub ctx: *mut std::ffi::c_void,
    pub query: Option<
        unsafe extern "C" fn(
            *mut std::ffi::c_void,
            *const VksInvR4bQuery,
            *mut VksInvR4bOut,
        ) -> i64,
    >,
    pub act: Option<unsafe extern "C" fn(*mut std::ffi::c_void, *const VksInvR4bAction)>,
    pub throttle: *mut VksInvR4bThrottle,
}

/// The answer sink of one query.
#[derive(Default)]
pub struct VksInvR4bOut {
    bytes: Vec<u8>,
    items: Vec<ModelItem>,
    cats: Vec<ModelCat>,
    objects: Vec<ViewerObject>,
    tasks: Vec<TaskObject>,
}

// Layout: the same numbers are static asserts in the header (64-bit).
const _: () = {
    use std::mem::{offset_of, size_of};
    assert!(size_of::<VksInvNetHost>() == 8);
    assert!(size_of::<VksInvR4bViewerObject>() == 37);
    if size_of::<usize>() == 8 {
        assert!(size_of::<VksInvOffer>() == 128 && offset_of!(VksInvOffer, from_name) == 80);
        assert!(offset_of!(VksInvOffer, host) == 112 && offset_of!(VksInvOffer, persist) == 120);
        assert!(size_of::<VksInvR4bItem>() == 168 && offset_of!(VksInvR4bItem, perms) == 72);
        assert!(size_of::<VksInvR4bCat>() == 56);
        assert!(size_of::<VksInvR4bTaskObject>() == 112);
        assert!(size_of::<VksInvR4bIm>() == 168 && offset_of!(VksInvR4bIm, from_agent_name) == 112);
        assert!(size_of::<VksInvR4bMove>() == 48);
        assert!(size_of::<VksInvR4bQuery>() == 80 && offset_of!(VksInvR4bQuery, list) == 64);
        assert!(size_of::<VksInvR4bAction>() == 184 && offset_of!(VksInvR4bAction, text) == 56);
        assert!(offset_of!(VksInvR4bAction, mv) == 176);
        assert!(size_of::<VksInvR4bThrottle>() == 16);
        assert!(size_of::<VksInvR4bHost>() == 32);
    }
};

// ---------------------------------------------------------------------------
// Codes (the header's enums)

mod q {
    pub const AGENT_ID: u32 = 1;
    pub const AGENT_SESSION_ID: u32 = 2;
    pub const AGENT_GROUP_ID: u32 = 3;
    pub const AGENT_POSITION: u32 = 4;
    pub const AGENT_REGION_HOST: u32 = 5;
    pub const IS_DO_NOT_DISTURB: u32 = 6;
    pub const HAVE_AVATAR: u32 = 7;
    pub const AVATAR_VALID: u32 = 8;
    pub const AVATAR_FULLNAME: u32 = 9;
    pub const IS_WEARING_ATTACHMENT: u32 = 10;
    pub const IS_WEARING_ITEM: u32 = 11;
    pub const IS_GESTURE_ACTIVE: u32 = 12;
    pub const AVATAR_SITTING: u32 = 13;
    pub const AVATAR_ROOT: u32 = 14;
    pub const SETTING_BOOL: u32 = 15;
    pub const SETTING_U32: u32 = 16;
    pub const AUTORESPONSE_TEXT: u32 = 17;
    pub const TRANS: u32 = 18;
    pub const SLURL_LOCATION: u32 = 19;
    pub const RLV_ANONYM: u32 = 20;
    pub const RLV_ANONYM_AVATAR: u32 = 21;
    pub const SLURL: u32 = 22;
    pub const APP_TITLE: u32 = 23;
    pub const TRIM_RESIDENT_SURNAME: u32 = 24;
    pub const RLV_ENABLED: u32 = 25;
    pub const RLV_CAN_SHOW_NAME: u32 = 26;
    pub const RLV_CAN_GIVE_INVENTORY: u32 = 27;
    pub const RLV_IS_NEARBY: u32 = 28;
    pub const RLV_HAS_OPEN_IM: u32 = 29;
    pub const RLV_HAS_OPEN_PROFILE: u32 = 30;
    pub const RLV_FORBID_GIVE_TO_RLV: u32 = 31;
    pub const RLV_HAS_SHARED_ROOT: u32 = 32;
    pub const RLV_SHARED_ROOT_ID: u32 = 33;
    pub const RLV_LOCKED_ATTACHMENT: u32 = 34;
    pub const RLV_HAS_BEHAVIOUR: u32 = 35;
    pub const IS_MUTED: u32 = 36;
    pub const AVATAR_USER_NAME: u32 = 37;
    pub const AVATAR_COMPLETE_NAME: u32 = 38;
    pub const GROUP_NAME: u32 = 39;
    pub const FIND_ID_BY_NAME: u32 = 40;
    pub const NOTIFICATION_LIVE: u32 = 41;
    pub const NOTIFICATION_FORM: u32 = 42;
    pub const ITEM: u32 = 43;
    pub const CATEGORY: u32 = 44;
    pub const CHILDREN: u32 = 45;
    pub const IS_CATEGORY_COMPLETE: u32 = 46;
    pub const FIND_CATEGORY_FOR_TYPE: u32 = 47;
    pub const GENERATE_UUID: u32 = 48;
    pub const NOW: u32 = 49;
    pub const FRAME_TIME: u32 = 50;
    pub const STARTUP_STARTED: u32 = 51;
    pub const IM_SESSION_NAME: u32 = 52;
    pub const MAX_ITEMS_TRANSFER: u32 = 53;
    pub const OBJECT: u32 = 54;
    pub const OBJECT_INVENTORY: u32 = 55;
    pub const HAVE_FLOATER_TOOLS: u32 = 56;
    pub const OBSERVER_NEW: u32 = 57;
    pub const OBSERVER_START: u32 = 58;
    pub const FILTER_CHECK: u32 = 59;
}

mod a {
    pub const OFFER_NEW: u32 = 1;
    pub const OFFER_UPDATE: u32 = 2;
    pub const OFFER_DELETE: u32 = 3;
    pub const SEND_RELIABLE: u32 = 4;
    pub const PACK_IM: u32 = 5;
    pub const SEND_PACKED_TO_REGION: u32 = 6;
    pub const NOTIFY: u32 = 7;
    pub const NOTIFY_POSTPONED: u32 = 8;
    pub const FORCE_RESPOND: u32 = 9;
    pub const AVATAR_NAME_LOOKUP: u32 = 10;
    pub const GROUP_NAME_LOOKUP: u32 = 11;
    pub const CACHE_NAME_GET: u32 = 12;
    pub const OBSERVER_DONE: u32 = 13;
    pub const OBSERVER_ADD: u32 = 14;
    pub const OBSERVER_DROP: u32 = 15;
    pub const RECENT_PEOPLE_ADD: u32 = 16;
    pub const FIRST_USE_NEW_INVENTORY: u32 = 17;
    pub const NEARBY_CHAT: u32 = 18;
    pub const UI_SOUND: u32 = 19;
    pub const INVENTORY_PANEL_SELECT: u32 = 20;
    pub const GROUP_PANEL_SHOW_NOTICE: u32 = 21;
    pub const RLV_SEND_NOTIFICATION: u32 = 22;
    pub const RLV_NOTIFY_BLOCKED_SHARE: u32 = 23;
    pub const POST_TO_MAIN_CORO: u32 = 24;
    pub const HUD_BEAM: u32 = 25;
    pub const FLOATER_TOOLS_DIRTY: u32 = 26;
    pub const MUTE_AUTO_REMOVE: u32 = 27;
    pub const IM_SYSTEM_MESSAGE: u32 = 28;
    pub const IM_LOG_TO_FILE: u32 = 29;
    pub const DELETE_OBJECT: u32 = 30;
    pub const NOTIFY_OBSERVERS: u32 = 31;
    pub const FLOATER_HIDE: u32 = 32;
    pub const WARN_MOVE_INVENTORY: u32 = 33;
    pub const MOVE_TASK_INVENTORY: u32 = 34;
    pub const LOG: u32 = 35;
}

const THEN_OFFER_MUTE_AVATAR: i32 = 0;
const THEN_OFFER_MUTE_GROUP: i32 = 1;
const THEN_SYSTEM_MESSAGE_TIP: i32 = 2;
const THEN_GROUP_NOTICE: i32 = 3;
const THEN_SHARE_CONFIRMATION: i32 = 4;
const THEN_AUTO_UNMUTE_NOTICE: i32 = 5;

const H_COPY_PROTECTED_ITEM: i32 = 1;
const H_COPY_PROTECTED_CATEGORY: i32 = 2;
const H_DRAG_GIVE_INVENTORY: i32 = 3;

// ---------------------------------------------------------------------------
// Conversions

const NO_BYTES: VksInvBytes = VksInvBytes {
    ptr: ptr::null(),
    len: 0,
};

fn view(b: &[u8]) -> VksInvBytes {
    VksInvBytes {
        ptr: b.as_ptr(),
        len: b.len(),
    }
}

unsafe fn get<'a>(b: &VksInvBytes) -> &'a [u8] {
    unsafe { bytes(b.ptr, b.len) }
}

/// Binary LLSD without a header; undefined is no bytes.
fn sd_out(v: &Value) -> Vec<u8> {
    if matches!(v, Value::Undefined) {
        Vec::new()
    } else {
        vkstorm_llsd::format(Format::Binary, v, Options::default()).0
    }
}

/// Binary LLSD from C++; no bytes (or nothing parsable) is undefined.
fn sd_in(b: &[u8]) -> Value {
    if b.is_empty() {
        return Value::Undefined;
    }
    vkstorm_llsd::parse(Format::Binary, b, Limits::for_len(b.len()))
        .1
        .unwrap_or(Value::Undefined)
}

unsafe fn sd_arg(p: *const u8, len: usize) -> Value {
    sd_in(unsafe { bytes(p, len) })
}

fn net_from(h: &VksInvNetHost) -> NetHost {
    NetHost {
        ip: h.ip,
        port: h.port,
    }
}

fn net_to(h: &NetHost) -> VksInvNetHost {
    VksInvNetHost {
        ip: h.ip,
        port: h.port,
    }
}

unsafe fn offer_from(o: &VksInvOffer) -> OfferInfo {
    OfferInfo {
        im: o.im,
        from_id: o.from_id,
        from_group: o.from_group,
        from_object: o.from_object,
        transaction_id: o.transaction_id,
        folder_id: o.folder_id,
        object_id: o.object_id,
        ty: AssetType(o.ty),
        from_name: unsafe { get(&o.from_name) }.to_vec(),
        desc: unsafe { get(&o.desc) }.to_vec(),
        host: net_from(&o.host),
        persist: o.persist,
    }
}

/// The view of `o` (its strings borrow `o`).
fn offer_to(o: &OfferInfo) -> VksInvOffer {
    VksInvOffer {
        im: o.im,
        from_id: o.from_id,
        from_group: o.from_group,
        from_object: o.from_object,
        transaction_id: o.transaction_id,
        folder_id: o.folder_id,
        object_id: o.object_id,
        ty: o.ty.0,
        from_name: view(&o.from_name),
        desc: view(&o.desc),
        host: net_to(&o.host),
        persist: o.persist,
    }
}

unsafe fn item_from(v: &VksInvR4bItem) -> ModelItem {
    ModelItem {
        id: v.id,
        parent: v.parent,
        ty: AssetType(v.ty),
        asset: v.asset,
        name: unsafe { get(&v.name) }.to_vec(),
        perm: v.perms,
        finished: v.finished,
    }
}

unsafe fn cat_from(v: &VksInvR4bCat) -> ModelCat {
    ModelCat {
        id: v.id,
        parent: v.parent,
        preferred: FolderType(v.preferred),
        name: unsafe { get(&v.name) }.to_vec(),
    }
}

fn object_from(v: &VksInvR4bViewerObject) -> ViewerObject {
    ViewerObject {
        id: v.id,
        perm_modify: v.perm_modify,
        flag_allow_inventory_add: v.flag_allow_inventory_add,
        perm_you_owner: v.perm_you_owner,
        pcode: v.pcode,
        is_attachment: v.is_attachment,
        root: v.root,
    }
}

unsafe fn opt_item(p: *const VksInvR4bItem) -> Option<ModelItem> {
    if p.is_null() {
        None
    } else {
        Some(unsafe { item_from(&*p) })
    }
}

unsafe fn opt_cat(p: *const VksInvR4bCat) -> Option<ModelCat> {
    if p.is_null() {
        None
    } else {
        Some(unsafe { cat_from(&*p) })
    }
}

unsafe fn uuid_list(p: *const u8, count: usize) -> Vec<Uuid> {
    let raw = unsafe { bytes(p, count.saturating_mul(16)) };
    raw.chunks_exact(16)
        .map(|c| {
            let mut u = [0u8; 16];
            u.copy_from_slice(c);
            u
        })
        .collect()
}

fn acceptance(a: Acceptance) -> i32 {
    a as i32
}

// ---------------------------------------------------------------------------
// The sink

/// # Safety
/// `out` is the sink of the running query; `data` holds `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_r4b_out_bytes(
    out: *mut VksInvR4bOut,
    data: *const u8,
    len: usize,
) {
    if let Some(o) = unsafe { out.as_mut() } {
        o.bytes.extend_from_slice(unsafe { bytes(data, len) });
    }
}

/// # Safety
/// `out` is the sink of the running query; `item` is a valid record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_r4b_out_item(out: *mut VksInvR4bOut, item: *const VksInvR4bItem) {
    if let (Some(o), Some(i)) = (unsafe { out.as_mut() }, unsafe { item.as_ref() }) {
        o.items.push(unsafe { item_from(i) });
    }
}

/// # Safety
/// `out` is the sink of the running query; `cat` is a valid record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_r4b_out_cat(out: *mut VksInvR4bOut, cat: *const VksInvR4bCat) {
    if let (Some(o), Some(c)) = (unsafe { out.as_mut() }, unsafe { cat.as_ref() }) {
        o.cats.push(unsafe { cat_from(c) });
    }
}

/// # Safety
/// `out` is the sink of the running query; `obj` is a valid record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_r4b_out_object(
    out: *mut VksInvR4bOut,
    obj: *const VksInvR4bViewerObject,
) {
    if let (Some(o), Some(v)) = (unsafe { out.as_mut() }, unsafe { obj.as_ref() }) {
        o.objects.push(object_from(v));
    }
}

/// # Safety
/// `out` is the sink of the running query; `obj` is a valid record.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_r4b_out_task_object(
    out: *mut VksInvR4bOut,
    obj: *const VksInvR4bTaskObject,
) {
    if let (Some(o), Some(v)) = (unsafe { out.as_mut() }, unsafe { obj.as_ref() }) {
        o.tasks.push(TaskObject {
            id: v.id,
            ty: AssetType(v.ty),
            perm: v.has_perms.then_some(v.perms),
        });
    }
}

// ---------------------------------------------------------------------------
// The host adapter

/// One query's arguments.
#[derive(Default)]
struct Ask<'a> {
    id: Option<&'a Uuid>,
    id2: Option<&'a Uuid>,
    text: &'a [u8],
    text2: &'a [u8],
    number: i32,
    flag: bool,
    list: Vec<VksInvBytes>,
}

fn id(u: &Uuid) -> Ask<'_> {
    Ask {
        id: Some(u),
        ..Ask::default()
    }
}

fn text(t: &[u8]) -> Ask<'_> {
    Ask {
        text: t,
        ..Ask::default()
    }
}

fn number<'a>(n: i32) -> Ask<'a> {
    Ask {
        number: n,
        ..Ask::default()
    }
}

/// [`Host`] over the caller's callbacks.
struct CHost {
    raw: VksInvR4bHost,
    /// The throttle state while the decision holds it (written back to the
    /// caller's before the next read and when the call ends).
    throttle: Option<Throttle>,
}

impl CHost {
    unsafe fn new(raw: *const VksInvR4bHost) -> Option<CHost> {
        let raw = unsafe { raw.as_ref() }.copied();
        match raw {
            Some(r) if r.query.is_some() && r.act.is_some() => Some(CHost {
                raw: r,
                throttle: None,
            }),
            _ => {
                set_error("vks_inv_r4b: no host");
                None
            }
        }
    }

    fn store_throttle(&mut self) {
        if let (Some(t), false) = (self.throttle.take(), self.raw.throttle.is_null()) {
            unsafe {
                *self.raw.throttle = VksInvR4bThrottle {
                    expiry: t.expiry,
                    count: t.count,
                    logged: t.logged,
                };
            }
        }
    }

    fn ask(&self, code: u32, a: Ask<'_>) -> (i64, VksInvR4bOut) {
        let q = VksInvR4bQuery {
            code,
            id: a.id.map_or(ptr::null(), |u| u.as_ptr()),
            id2: a.id2.map_or(ptr::null(), |u| u.as_ptr()),
            text: view(a.text),
            text2: view(a.text2),
            number: a.number,
            flag: a.flag,
            list: if a.list.is_empty() {
                ptr::null()
            } else {
                a.list.as_ptr()
            },
            list_len: a.list.len(),
        };
        let mut out = VksInvR4bOut::default();
        let f = self.raw.query.expect("checked in new");
        let r = unsafe { f(self.raw.ctx, &q, &mut out) };
        (r, out)
    }

    fn flag(&self, code: u32, a: Ask<'_>) -> bool {
        self.ask(code, a).0 != 0
    }

    fn bytes(&self, code: u32, a: Ask<'_>) -> Vec<u8> {
        self.ask(code, a).1.bytes
    }

    fn found_bytes(&self, code: u32, a: Ask<'_>) -> Option<Vec<u8>> {
        let (r, out) = self.ask(code, a);
        (r != 0).then_some(out.bytes)
    }

    fn uuid(&self, code: u32, a: Ask<'_>) -> Uuid {
        let b = self.bytes(code, a);
        let mut u = [0u8; 16];
        if b.len() >= 16 {
            u.copy_from_slice(&b[..16]);
        }
        u
    }

    fn double(&self, code: u32) -> f64 {
        let b = self.bytes(code, Ask::default());
        b.get(..8)
            .map_or(0.0, |s| f64::from_ne_bytes(s.try_into().unwrap()))
    }

    fn send(&self, a: VksInvR4bAction) {
        let f = self.raw.act.expect("checked in new");
        unsafe { f(self.raw.ctx, &a) };
    }
}

impl Drop for CHost {
    fn drop(&mut self) {
        self.store_throttle();
    }
}

fn blank(kind: u32) -> VksInvR4bAction {
    VksInvR4bAction {
        kind,
        number: [0; 3],
        flag: [false; 2],
        id: [[0; 16]; 2],
        text: [NO_BYTES; 4],
        sd: [NO_BYTES; 2],
        offer: ptr::null(),
        im: ptr::null(),
        host: VksInvNetHost::default(),
        mv: ptr::null(),
    }
}

fn im_view(p: &ImPacket) -> VksInvR4bIm {
    VksInvR4bIm {
        agent_id: p.agent_id,
        session_id: p.session_id,
        from_group: p.from_group,
        to_agent_id: p.to_agent_id,
        parent_estate_id: p.parent_estate_id,
        region_id: p.region_id,
        position: p.position,
        offline: p.offline,
        dialog: p.dialog as i32,
        id: p.id,
        timestamp: p.timestamp,
        from_agent_name: view(&p.from_agent_name),
        message: view(&p.message),
        bucket: view(&p.binary_bucket),
        has_bucket: true,
    }
}

fn pack_view(p: &PackArgs) -> VksInvR4bIm {
    VksInvR4bIm {
        agent_id: p.from_id,
        session_id: p.session_id,
        from_group: p.from_group,
        to_agent_id: p.to_id,
        parent_estate_id: p.parent_estate_id,
        region_id: p.region_id,
        position: p.position,
        offline: p.offline,
        dialog: p.dialog,
        id: p.id,
        timestamp: p.timestamp,
        from_agent_name: view(&p.name),
        message: view(&p.message),
        bucket: p.bucket.as_deref().map_or(NO_BYTES, view),
        has_bucket: p.bucket.is_some(),
    }
}

fn offline_view(m: &OfflineMessage) -> VksInvR4bIm {
    VksInvR4bIm {
        agent_id: m.from_id,
        session_id: m.session_id,
        from_group: m.from_group,
        to_agent_id: m.to_id,
        parent_estate_id: m.parent_estate_id,
        region_id: m.region_id,
        position: m.position,
        offline: m.offline,
        dialog: m.dialog,
        id: [0; 16],
        timestamp: m.timestamp,
        from_agent_name: view(&m.name),
        message: view(&m.message),
        bucket: view(&m.bucket),
        has_bucket: true,
    }
}

fn move_pairs(m: &MoveInv) -> Vec<u8> {
    let mut v = Vec::with_capacity(m.moves.len() * 32);
    for (c, i) in &m.moves {
        v.extend_from_slice(c);
        v.extend_from_slice(i);
    }
    v
}

fn move_view(m: &MoveInv, pairs: &[u8]) -> VksInvR4bMove {
    VksInvR4bMove {
        object_id: m.object_id,
        category_id: m.category_id,
        pairs: if pairs.is_empty() {
            ptr::null()
        } else {
            pairs.as_ptr()
        },
        count: m.moves.len(),
    }
}

impl CHost {
    /// A name lookup action with its continuation.
    fn lookup(&self, kind: u32, target: Uuid, is_group: bool, then: &NameThen) {
        let mut a = blank(kind);
        a.id[0] = target;
        a.flag[0] = is_group;
        let mut sd1 = Vec::new();
        match then {
            NameThen::OfferMuteAvatar => a.number[1] = THEN_OFFER_MUTE_AVATAR,
            NameThen::OfferMuteGroup => a.number[1] = THEN_OFFER_MUTE_GROUP,
            NameThen::SystemMessageTip { format, message } => {
                a.number[1] = THEN_SYSTEM_MESSAGE_TIP;
                a.text[1] = view(format);
                a.text[2] = view(message);
            }
            NameThen::GroupNotice {
                sender,
                subject,
                message,
                payload,
                timestamp,
            } => {
                a.number[1] = THEN_GROUP_NOTICE;
                a.text[1] = view(sender);
                a.text[2] = view(subject);
                a.text[3] = view(message);
                sd1 = sd_out(payload);
                a.number[2] = *timestamp as i32;
            }
            NameThen::ShareConfirmation {
                item_id,
                item_name,
                dest,
            } => {
                a.number[1] = THEN_SHARE_CONFIRMATION;
                a.id[1] = *item_id;
                a.text[1] = view(item_name);
                a.text[2] = view(dest);
            }
            NameThen::AutoUnmuteNotice { reason } => {
                a.number[1] = THEN_AUTO_UNMUTE_NOTICE;
                a.number[2] = *reason;
            }
        }
        a.sd[1] = view(&sd1);
        self.send(a);
    }
}

fn kind_observer(k: &ObserverKind) -> (i32, Uuid, Uuid, &[u8], bool) {
    const N: Uuid = [0; 16];
    match k {
        ObserverKind::FetchItems { id } => (0, *id, N, &[], false),
        ObserverKind::OpenAgentOffer {
            id,
            from_name,
            manual,
        } => (1, *id, N, from_name, *manual),
        ObserverKind::DiscardAgentOffer { folder, object } => (2, *folder, *object, &[], false),
        ObserverKind::RlvGiveToRlvAgentOffer { folder } => (3, *folder, N, &[], false),
        ObserverKind::RlvGiveToRlvTaskOffer { transaction } => (4, *transaction, N, &[], false),
        ObserverKind::OpenTaskGroupOffer => (5, N, N, &[], false),
    }
}

impl Host for CHost {
    fn act(&mut self, action: Action) {
        // Strings and LLSD the action borrows live in these locals until
        // the callback returns.
        match &action {
            Action::OfferNew(o) | Action::OfferUpdate(o) => {
                let kind = if matches!(action, Action::OfferNew(_)) {
                    a::OFFER_NEW
                } else {
                    a::OFFER_UPDATE
                };
                let v = offer_to(o);
                let mut x = blank(kind);
                x.offer = &v;
                self.send(x);
            }
            Action::OfferDelete => self.send(blank(a::OFFER_DELETE)),
            Action::SendReliable { packet, route } => {
                let v = im_view(packet);
                let mut x = blank(a::SEND_RELIABLE);
                x.im = &v;
                match route {
                    Route::Host(h) => x.host = net_to(h),
                    Route::AgentRegion => x.number[0] = 1,
                }
                self.send(x);
            }
            Action::PackInstantMessage(args) => {
                let v = pack_view(args);
                let mut x = blank(a::PACK_IM);
                x.im = &v;
                self.send(x);
            }
            Action::SendPackedToRegion => self.send(blank(a::SEND_PACKED_TO_REGION)),
            Action::Notify {
                name,
                substitutions,
                payload,
                handler,
            } => {
                let s0 = sd_out(substitutions);
                let s1 = sd_out(payload);
                let mut x = blank(a::NOTIFY);
                x.text[0] = view(name.as_bytes());
                x.sd = [view(&s0), view(&s1)];
                x.number[0] = match handler {
                    None => 0,
                    Some(Handler::CopyProtectedItem) => H_COPY_PROTECTED_ITEM,
                    Some(Handler::CopyProtectedCategory) => H_COPY_PROTECTED_CATEGORY,
                    Some(Handler::DragGiveInventory) => H_DRAG_GIVE_INVENTORY,
                };
                self.send(x);
            }
            Action::NotifyPostponed {
                name,
                substitutions,
                payload,
                offer_from_agent,
                from_id,
                is_group,
            } => {
                let s0 = sd_out(substitutions);
                let s1 = sd_out(payload);
                let mut x = blank(a::NOTIFY_POSTPONED);
                x.text[0] = view(name.as_bytes());
                x.sd = [view(&s0), view(&s1)];
                x.flag = [*offer_from_agent, *is_group];
                x.id[0] = *from_id;
                self.send(x);
            }
            Action::ForceRespond { name, response } => {
                let s0 = sd_out(response);
                let mut x = blank(a::FORCE_RESPOND);
                x.text[0] = view(name.as_bytes());
                x.sd[0] = view(&s0);
                self.send(x);
            }
            Action::AvatarNameLookup { id, then } => {
                self.lookup(a::AVATAR_NAME_LOOKUP, *id, false, then)
            }
            Action::GroupNameLookup { id, then } => {
                self.lookup(a::GROUP_NAME_LOOKUP, *id, true, then)
            }
            Action::CacheNameGet { id, is_group, then } => {
                self.lookup(a::CACHE_NAME_GET, *id, *is_group, then)
            }
            Action::ObserverDone(o) | Action::ObserverAdd(o) | Action::ObserverDrop(o) => {
                let kind = match action {
                    Action::ObserverDone(_) => a::OBSERVER_DONE,
                    Action::ObserverAdd(_) => a::OBSERVER_ADD,
                    _ => a::OBSERVER_DROP,
                };
                let mut x = blank(kind);
                x.number[0] = *o as i32;
                self.send(x);
            }
            Action::RecentPeopleAdd(id) => {
                let mut x = blank(a::RECENT_PEOPLE_ADD);
                x.id[0] = *id;
                self.send(x);
            }
            Action::FirstUseNewInventory => self.send(blank(a::FIRST_USE_NEW_INVENTORY)),
            Action::NearbyChat(m) => {
                let mut x = blank(a::NEARBY_CHAT);
                x.text[0] = view(m);
                self.send(x);
            }
            Action::UiSound(n) => {
                let mut x = blank(a::UI_SOUND);
                x.text[0] = view(n.as_bytes());
                self.send(x);
            }
            Action::InventoryPanelSelect { auto_open, id } => {
                let mut x = blank(a::INVENTORY_PANEL_SELECT);
                x.flag[0] = *auto_open;
                x.id[0] = *id;
                self.send(x);
            }
            Action::GroupPanelShowNotice {
                subject,
                message,
                group_id,
                has_inventory,
                inventory_name,
                with_offer,
            } => {
                let mut x = blank(a::GROUP_PANEL_SHOW_NOTICE);
                x.text[0] = view(subject);
                x.text[1] = view(message);
                x.text[2] = view(inventory_name);
                x.id[0] = *group_id;
                x.flag = [*has_inventory, *with_offer];
                self.send(x);
            }
            Action::RlvSendNotification(t) => {
                let mut x = blank(a::RLV_SEND_NOTIFICATION);
                x.text[0] = view(t);
                self.send(x);
            }
            Action::RlvNotifyBlockedShare { args } => {
                let s0 = sd_out(args);
                let mut x = blank(a::RLV_NOTIFY_BLOCKED_SHARE);
                x.sd[0] = view(&s0);
                self.send(x);
            }
            Action::PostToMainCoro(m) => {
                let v = offline_view(m);
                let s0 = sd_out(&m.metadata);
                let mut x = blank(a::POST_TO_MAIN_CORO);
                x.im = &v;
                x.host = net_to(&m.sender);
                x.sd[0] = view(&s0);
                x.id[0] = m.aux_id;
                self.send(x);
            }
            Action::HudBeam { to } => {
                let mut x = blank(a::HUD_BEAM);
                x.id[0] = *to;
                self.send(x);
            }
            Action::FloaterToolsDirty => self.send(blank(a::FLOATER_TOOLS_DIRTY)),
            Action::MuteAutoRemove { id, reason } => {
                let mut x = blank(a::MUTE_AUTO_REMOVE);
                x.id[0] = *id;
                x.number[0] = *reason;
                self.send(x);
            }
            Action::ImSystemMessage {
                session,
                name,
                args,
            } => {
                let s0 = sd_out(args);
                let mut x = blank(a::IM_SYSTEM_MESSAGE);
                x.id[0] = *session;
                x.text[0] = view(name.as_bytes());
                x.sd[0] = view(&s0);
                self.send(x);
            }
            Action::ImLogToFile {
                file,
                from,
                id,
                template,
                args,
            } => {
                let s0 = sd_out(args);
                let mut x = blank(a::IM_LOG_TO_FILE);
                x.text[0] = view(file);
                x.text[1] = view(from);
                x.text[2] = view(template);
                x.id[0] = *id;
                x.sd[0] = view(&s0);
                self.send(x);
            }
            Action::DeleteObject(id) => {
                let mut x = blank(a::DELETE_OBJECT);
                x.id[0] = *id;
                self.send(x);
            }
            Action::NotifyObservers => self.send(blank(a::NOTIFY_OBSERVERS)),
            Action::FloaterHide(n) => {
                let mut x = blank(a::FLOATER_HIDE);
                x.text[0] = view(n.as_bytes());
                self.send(x);
            }
            Action::WarnMoveInventory(mv) => {
                let pairs = move_pairs(mv);
                let v = move_view(mv, &pairs);
                let mut x = blank(a::WARN_MOVE_INVENTORY);
                x.mv = &v;
                self.send(x);
            }
            Action::MoveTaskInventory {
                notification,
                response,
                mv,
            } => {
                let pairs = move_pairs(mv);
                let v = move_view(mv, &pairs);
                let s0 = sd_out(notification);
                let s1 = sd_out(response);
                let mut x = blank(a::MOVE_TASK_INVENTORY);
                x.mv = &v;
                x.sd = [view(&s0), view(&s1)];
                self.send(x);
            }
            Action::Log {
                level,
                tag,
                message,
            } => {
                let mut x = blank(a::LOG);
                x.number[0] = match level {
                    LogLevel::Debug => 0,
                    LogLevel::Info => 1,
                    LogLevel::Warn => 2,
                };
                x.text[0] = view(tag.as_bytes());
                x.text[1] = view(message);
                self.send(x);
            }
        }
    }

    fn agent_id(&self) -> Uuid {
        self.uuid(q::AGENT_ID, Ask::default())
    }
    fn agent_session_id(&self) -> Uuid {
        self.uuid(q::AGENT_SESSION_ID, Ask::default())
    }
    fn agent_group_id(&self) -> Uuid {
        self.uuid(q::AGENT_GROUP_ID, Ask::default())
    }
    fn agent_position(&self) -> [f32; 3] {
        let b = self.bytes(q::AGENT_POSITION, Ask::default());
        let mut p = [0.0f32; 3];
        for (i, v) in p.iter_mut().enumerate() {
            if let Some(s) = b.get(i * 4..i * 4 + 4) {
                *v = f32::from_ne_bytes(s.try_into().unwrap());
            }
        }
        p
    }
    fn agent_region_host(&self) -> NetHost {
        let b = self.bytes(q::AGENT_REGION_HOST, Ask::default());
        if b.len() < 8 {
            return NetHost::default();
        }
        NetHost {
            ip: [b[0], b[1], b[2], b[3]],
            port: u32::from_ne_bytes(b[4..8].try_into().unwrap()),
        }
    }
    fn is_do_not_disturb(&self) -> bool {
        self.flag(q::IS_DO_NOT_DISTURB, Ask::default())
    }
    fn have_avatar(&self) -> bool {
        self.flag(q::HAVE_AVATAR, Ask::default())
    }
    fn avatar_valid(&self) -> bool {
        self.flag(q::AVATAR_VALID, Ask::default())
    }
    fn avatar_fullname(&self) -> Vec<u8> {
        self.bytes(q::AVATAR_FULLNAME, Ask::default())
    }
    fn is_wearing_attachment(&self, i: &Uuid) -> bool {
        self.flag(q::IS_WEARING_ATTACHMENT, id(i))
    }
    fn is_wearing_item(&self, i: &Uuid) -> bool {
        self.flag(q::IS_WEARING_ITEM, id(i))
    }
    fn is_gesture_active(&self, i: &Uuid) -> bool {
        self.flag(q::IS_GESTURE_ACTIVE, id(i))
    }
    fn avatar_sitting(&self) -> bool {
        self.flag(q::AVATAR_SITTING, Ask::default())
    }
    fn avatar_root(&self) -> Option<Uuid> {
        let b = self.found_bytes(q::AVATAR_ROOT, Ask::default())?;
        let mut u = [0u8; 16];
        if b.len() >= 16 {
            u.copy_from_slice(&b[..16]);
        }
        Some(u)
    }

    fn setting_bool(&self, name: &str) -> bool {
        self.flag(q::SETTING_BOOL, text(name.as_bytes()))
    }
    fn setting_u32(&self, name: &str) -> u32 {
        self.ask(q::SETTING_U32, text(name.as_bytes())).0 as u32
    }
    fn autoresponse_text(&self, from: &Uuid) -> Vec<u8> {
        self.bytes(q::AUTORESPONSE_TEXT, id(from))
    }

    fn trans(&self, key: &str, args: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut sorted: Vec<&(&str, Vec<u8>)> = args.iter().collect();
        sorted.sort_by(|x, y| x.0.as_bytes().cmp(y.0.as_bytes()));
        let mut list = Vec::with_capacity(sorted.len() * 2);
        for (k, v) in sorted {
            list.push(view(k.as_bytes()));
            list.push(view(v));
        }
        self.bytes(
            q::TRANS,
            Ask {
                text: key.as_bytes(),
                list,
                ..Ask::default()
            },
        )
    }
    fn slurl_location(&self, url: &[u8]) -> Vec<u8> {
        self.bytes(q::SLURL_LOCATION, text(url))
    }
    fn rlv_anonym(&self, name: &[u8]) -> Vec<u8> {
        self.bytes(q::RLV_ANONYM, text(name))
    }
    fn rlv_anonym_avatar(&self, i: &Uuid) -> Option<Vec<u8>> {
        self.found_bytes(q::RLV_ANONYM_AVATAR, id(i))
    }
    fn slurl(&self, command: &str, i: &Uuid, verb: &[u8]) -> Vec<u8> {
        self.bytes(
            q::SLURL,
            Ask {
                text: command.as_bytes(),
                id: Some(i),
                text2: verb,
                ..Ask::default()
            },
        )
    }
    fn trans_app_title(&self) -> Vec<u8> {
        self.bytes(q::APP_TITLE, Ask::default())
    }
    fn trim_resident_surname(&self) -> bool {
        self.flag(q::TRIM_RESIDENT_SURNAME, Ask::default())
    }

    fn rlv_enabled(&self) -> bool {
        self.flag(q::RLV_ENABLED, Ask::default())
    }
    fn rlv_can_show_name(&self, i: &Uuid) -> bool {
        self.flag(q::RLV_CAN_SHOW_NAME, id(i))
    }
    fn rlv_can_give_inventory(&self, i: &Uuid) -> bool {
        self.flag(q::RLV_CAN_GIVE_INVENTORY, id(i))
    }
    fn rlv_is_nearby(&self, i: &Uuid) -> bool {
        self.flag(q::RLV_IS_NEARBY, id(i))
    }
    fn rlv_has_open_im(&self, i: &Uuid) -> bool {
        self.flag(q::RLV_HAS_OPEN_IM, id(i))
    }
    fn rlv_has_open_profile(&self, i: &Uuid) -> bool {
        self.flag(q::RLV_HAS_OPEN_PROFILE, id(i))
    }
    fn rlv_forbid_give_to_rlv(&self) -> bool {
        self.flag(q::RLV_FORBID_GIVE_TO_RLV, Ask::default())
    }
    fn rlv_has_shared_root(&self) -> bool {
        self.flag(q::RLV_HAS_SHARED_ROOT, Ask::default())
    }
    fn rlv_shared_root_id(&self) -> Uuid {
        self.uuid(q::RLV_SHARED_ROOT_ID, Ask::default())
    }
    fn rlv_locked_attachment(&self, root: &Uuid) -> bool {
        self.flag(q::RLV_LOCKED_ATTACHMENT, id(root))
    }
    fn rlv_has_behaviour(&self, b: RlvBehaviour) -> bool {
        let n = match b {
            RlvBehaviour::Unsit => 0,
            RlvBehaviour::SitTp => 1,
        };
        self.flag(q::RLV_HAS_BEHAVIOUR, number(n))
    }

    fn is_muted(&self, i: &Uuid, name: &[u8], flags: u32) -> bool {
        self.flag(
            q::IS_MUTED,
            Ask {
                id: Some(i),
                text: name,
                number: flags as i32,
                ..Ask::default()
            },
        )
    }
    fn avatar_user_name(&self, i: &Uuid) -> Option<Vec<u8>> {
        self.found_bytes(q::AVATAR_USER_NAME, id(i))
    }
    fn avatar_complete_name(&self, i: &Uuid) -> Option<Vec<u8>> {
        self.found_bytes(q::AVATAR_COMPLETE_NAME, id(i))
    }
    fn group_name(&self, i: &Uuid) -> Option<Vec<u8>> {
        self.found_bytes(q::GROUP_NAME, id(i))
    }
    fn find_id_by_name(&self, name: &[u8]) -> Uuid {
        self.uuid(q::FIND_ID_BY_NAME, text(name))
    }

    fn notification_live(&self, i: &Uuid) -> bool {
        self.flag(q::NOTIFICATION_LIVE, id(i))
    }
    fn notification_form(&self, name: &str) -> Option<Value> {
        self.found_bytes(q::NOTIFICATION_FORM, text(name.as_bytes()))
            .map(|b| sd_in(&b))
    }

    fn item(&self, i: &Uuid) -> Option<ModelItem> {
        let (r, out) = self.ask(q::ITEM, id(i));
        if r == 0 {
            return None;
        }
        out.items.into_iter().next()
    }
    fn category(&self, i: &Uuid) -> Option<ModelCat> {
        let (r, out) = self.ask(q::CATEGORY, id(i));
        if r == 0 {
            return None;
        }
        out.cats.into_iter().next()
    }
    fn children(&self, parent: &Uuid) -> (Vec<ModelCat>, Vec<ModelItem>) {
        let out = self.ask(q::CHILDREN, id(parent)).1;
        (out.cats, out.items)
    }
    fn is_category_complete(&self, i: &Uuid) -> bool {
        self.flag(q::IS_CATEGORY_COMPLETE, id(i))
    }
    fn find_category_for_type(&mut self, ty: FolderType) -> Uuid {
        self.uuid(q::FIND_CATEGORY_FOR_TYPE, number(ty.0))
    }

    fn generate_uuid(&mut self) -> Uuid {
        self.uuid(q::GENERATE_UUID, Ask::default())
    }
    fn now(&self) -> f64 {
        self.double(q::NOW)
    }
    fn frame_time(&self) -> f64 {
        self.double(q::FRAME_TIME)
    }
    fn startup_started(&self) -> bool {
        self.flag(q::STARTUP_STARTED, Ask::default())
    }
    fn throttle(&mut self) -> &mut Throttle {
        self.store_throttle();
        let t =
            unsafe { self.raw.throttle.as_ref() }.map_or_else(Throttle::default, |t| Throttle {
                expiry: t.expiry,
                count: t.count,
                logged: t.logged,
            });
        self.throttle.insert(t)
    }
    fn im_session_name(&self, session: &Uuid) -> Option<Vec<u8>> {
        self.found_bytes(q::IM_SESSION_NAME, id(session))
    }
    fn max_inventory_items_transfer(&self) -> i32 {
        self.ask(q::MAX_ITEMS_TRANSFER, Ask::default()).0 as i32
    }
    fn object(&self, i: &Uuid) -> Option<ViewerObject> {
        let (r, out) = self.ask(q::OBJECT, id(i));
        if r == 0 {
            return None;
        }
        out.objects.into_iter().next()
    }
    fn object_inventory(&self, i: &Uuid) -> Vec<TaskObject> {
        self.ask(q::OBJECT_INVENTORY, id(i)).1.tasks
    }
    fn have_floater_tools(&self) -> bool {
        self.flag(q::HAVE_FLOATER_TOOLS, Ask::default())
    }

    fn observer_new(&mut self, kind: ObserverKind) -> ObserverId {
        let (n, i1, i2, name, manual) = kind_observer(&kind);
        self.ask(
            q::OBSERVER_NEW,
            Ask {
                number: n,
                id: Some(&i1),
                id2: Some(&i2),
                text: name,
                flag: manual,
                ..Ask::default()
            },
        )
        .0 as ObserverId
    }
    fn observer_start(&mut self, observer: ObserverId) -> bool {
        self.flag(q::OBSERVER_START, number(observer as i32))
    }
}

// ---------------------------------------------------------------------------
// Offers (W6)

/// Runs `f` with the host adapter of `host`, or returns `fail` (no host or a
/// panic).
unsafe fn with_host<T: Copy>(
    host: *const VksInvR4bHost,
    fail: T,
    f: impl FnOnce(&mut CHost) -> T,
) -> T {
    guard(fail, || match unsafe { CHost::new(host) } {
        Some(mut h) => f(&mut h),
        None => fail,
    })
}

/// # Safety
/// `host` is a valid host; the pointers hold the lengths given (16 bytes for
/// the ids; null ids read as the null UUID).
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn vks_inv_r4b_receive_im(
    host: *const VksInvR4bHost,
    from_id: *const u8,
    from_group: bool,
    dialog: i32,
    session_id: *const u8,
    timestamp: u32,
    name: *const u8,
    name_len: usize,
    message: *const u8,
    message_len: usize,
    buffer: *const u8,
    buffer_len: usize,
    bucket_size: i32,
    sender: *const VksInvNetHost,
    aux_id: *const u8,
) -> bool {
    unsafe {
        with_host(host, true, |h| {
            let args = ImArgs {
                from_id: uuid_arg(from_id),
                from_group,
                dialog,
                session_id: uuid_arg(session_id),
                timestamp,
                name: bytes(name, name_len),
                message: bytes(message, message_len),
                buffer: bytes(buffer, buffer_len),
                bucket_size,
                sender: sender.as_ref().map_or_else(NetHost::default, net_from),
                aux_id: uuid_arg(aux_id),
            };
            offer::receive_im(h, &args)
        })
    }
}

/// # Safety
/// `host` is a valid host; `messages` holds `len` bytes of binary LLSD.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_r4b_receive_offline(
    host: *const VksInvR4bHost,
    messages: *const u8,
    len: usize,
) {
    unsafe {
        with_host(host, (), |h| {
            let m = sd_arg(messages, len);
            offer::receive_offline(h, &m);
        })
    }
}

/// # Safety
/// `host` and `offer` are valid; the LLSD pointers hold their lengths.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_r4b_respond(
    host: *const VksInvR4bHost,
    offer: *const VksInvOffer,
    notification: *const u8,
    notification_len: usize,
    response: *const u8,
    response_len: usize,
) {
    unsafe {
        with_host(host, (), |h| {
            let Some(o) = offer.as_ref() else { return };
            let mut info = offer_from(o);
            let n = sd_arg(notification, notification_len);
            let r = sd_arg(response, response_len);
            offer::respond(h, &mut info, &n, &r);
        })
    }
}

/// # Safety
/// `host` and `offer` are valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_r4b_force_response(
    host: *const VksInvR4bHost,
    offer: *const VksInvOffer,
    response: i32,
) {
    unsafe {
        with_host(host, (), |h| {
            let Some(o) = offer.as_ref() else { return };
            let mut info = offer_from(o);
            offer::force_response(h, &mut info, response);
        })
    }
}

/// # Safety
/// `host` and `offer` are valid; `folder` is 16 bytes or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_r4b_send_receive_response(
    host: *const VksInvR4bHost,
    offer: *const VksInvOffer,
    accept: bool,
    folder: *const u8,
) {
    unsafe {
        with_host(host, (), |h| {
            let Some(o) = offer.as_ref() else { return };
            let mut info = offer_from(o);
            let folder = uuid_arg(folder);
            offer::send_receive_response(h, &mut info, accept, &folder);
        })
    }
}

/// # Safety
/// `host` and `offer` are valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_r4b_send_auto_receive_response(
    host: *const VksInvR4bHost,
    offer: *const VksInvOffer,
) {
    unsafe {
        with_host(host, (), |h| {
            let Some(o) = offer.as_ref() else { return };
            let mut info = offer_from(o);
            offer::send_auto_receive_response(h, &mut info);
        })
    }
}

/// # Safety
/// `host` and `offer` are valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_r4b_send_decline_response(
    host: *const VksInvR4bHost,
    offer: *const VksInvOffer,
) {
    unsafe {
        with_host(host, (), |h| {
            let Some(o) = offer.as_ref() else { return };
            let mut info = offer_from(o);
            offer::send_decline_response(h, &mut info);
        })
    }
}

/// # Safety
/// `host` is valid; `from_name` holds `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_r4b_check_offer_throttle(
    host: *const VksInvR4bHost,
    from_name: *const u8,
    len: usize,
    check_only: bool,
) -> bool {
    unsafe {
        with_host(host, false, |h| {
            throttle::check_offer_throttle(h, bytes(from_name, len), check_only)
        })
    }
}

// ---------------------------------------------------------------------------
// Giving (W7)

/// # Safety
/// `host` is valid; ids are 16 bytes or null; `item` is valid or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_r4b_give_item(
    host: *const VksInvR4bHost,
    to: *const u8,
    item: *const VksInvR4bItem,
    im_session: *const u8,
) -> bool {
    unsafe {
        with_host(host, false, |h| {
            let item = opt_item(item);
            give::give_item(h, &uuid_arg(to), item.as_ref(), &uuid_arg(im_session))
        })
    }
}

/// # Safety
/// `host` is valid; ids are 16 bytes or null; `cat` is valid or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_r4b_give_category(
    host: *const VksInvR4bHost,
    to: *const u8,
    cat: *const VksInvR4bCat,
    im_session: *const u8,
    success_notification: *const u8,
    len: usize,
) -> bool {
    unsafe {
        with_host(host, false, |h| {
            let cat = opt_cat(cat);
            give::give_category(
                h,
                &uuid_arg(to),
                cat.as_ref(),
                &uuid_arg(im_session),
                bytes(success_notification, len),
            )
        })
    }
}

/// # Safety
/// `host` is valid; the LLSD pointers hold their lengths.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_r4b_answer(
    host: *const VksInvR4bHost,
    handler: i32,
    notification: *const u8,
    notification_len: usize,
    response: *const u8,
    response_len: usize,
) -> bool {
    unsafe {
        with_host(host, false, |h| {
            let handler = match handler {
                H_COPY_PROTECTED_ITEM => Handler::CopyProtectedItem,
                H_COPY_PROTECTED_CATEGORY => Handler::CopyProtectedCategory,
                H_DRAG_GIVE_INVENTORY => Handler::DragGiveInventory,
                _ => {
                    set_error("vks_inv_r4b_answer: unknown handler");
                    return false;
                }
            };
            let n = sd_arg(notification, notification_len);
            let r = sd_arg(response, response_len);
            give::answer(h, handler, &n, &r)
        })
    }
}

/// # Safety
/// `host` is valid; `avatars` and `selected` hold their counts of 16-byte
/// ids; the LLSD pointers hold their lengths.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn vks_inv_r4b_share(
    host: *const VksInvR4bHost,
    avatars: *const u8,
    avatar_count: usize,
    notification: *const u8,
    notification_len: usize,
    response: *const u8,
    response_len: usize,
    selected: *const u8,
    selected_count: usize,
) {
    unsafe {
        with_host(host, (), |h| {
            let av = uuid_list(avatars, avatar_count);
            let sel = uuid_list(selected, selected_count);
            let n = sd_arg(notification, notification_len);
            let r = sd_arg(response, response_len);
            give::share(h, &av, &n, &r, &sel);
        })
    }
}

/// # Safety
/// `host` is valid; `item` is valid or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_r4b_is_give_acceptable(
    host: *const VksInvR4bHost,
    item: *const VksInvR4bItem,
) -> bool {
    unsafe {
        with_host(host, false, |h| {
            give::is_give_acceptable(h, opt_item(item).as_ref())
        })
    }
}

/// # Safety
/// `host` is valid; `item` is valid or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_r4b_is_group_give_acceptable(
    host: *const VksInvR4bHost,
    item: *const VksInvR4bItem,
) -> bool {
    unsafe {
        with_host(host, false, |h| {
            give::is_group_give_acceptable(h, opt_item(item).as_ref())
        })
    }
}

/// # Safety
/// `host` is valid; `item` is valid or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_r4b_transfer_commonly_allowed(
    host: *const VksInvR4bHost,
    item: *const VksInvR4bItem,
) -> bool {
    unsafe {
        with_host(host, false, |h| {
            give::transfer_commonly_allowed(h, opt_item(item).as_ref())
        })
    }
}

/// # Safety
/// `host` is valid; `cat` and `item` are valid or null; `count` is writable
/// or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_r4b_collect_functor(
    host: *const VksInvR4bHost,
    functor: i32,
    cat: *const VksInvR4bCat,
    item: *const VksInvR4bItem,
    count: *mut i32,
) -> bool {
    unsafe {
        with_host(host, false, |h| {
            let f = match functor {
                0 => Functor::Giveable,
                1 => Functor::Uncopyable,
                2 => Functor::DroppableTransfer,
                3 => Functor::DroppableNoTransfer,
                4 => Functor::DropCopyable,
                _ => {
                    set_error("vks_inv_r4b_collect_functor: unknown functor");
                    return false;
                }
            };
            let (c, i) = (opt_cat(cat), opt_item(item));
            let (r, n) = give::collect_functor(h, f, c.as_ref(), i.as_ref());
            if let Some(out) = count.as_mut() {
                *out = n.unwrap_or(-1);
            }
            r
        })
    }
}

/// # Safety
/// `host` is valid; `obj` and `item` are valid or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_r4b_will_object_accept(
    host: *const VksInvR4bHost,
    obj: *const VksInvR4bViewerObject,
    item: *const VksInvR4bItem,
    dad_type: i32,
) -> i32 {
    unsafe {
        with_host(host, acceptance(Acceptance::No), |h| {
            let o = obj.as_ref().map(object_from);
            let i = opt_item(item);
            acceptance(give::will_object_accept(
                h,
                o.as_ref(),
                i.as_ref(),
                dad_type,
            ))
        })
    }
}

/// `move_inv_category_world_to_agent`'s filter: asks the host with the
/// entry's index in the object's (unfiltered) inventory list, read once.
struct HostFilter {
    raw: VksInvR4bHost,
    object_id: Uuid,
    types: u64,
    list: Option<Vec<Uuid>>,
}

impl ItemFilter for HostFilter {
    fn filter_types(&self) -> u64 {
        self.types
    }

    fn check(&mut self, item: &TaskObject) -> bool {
        let h = CHost {
            raw: self.raw,
            throttle: None,
        };
        let object_id = self.object_id;
        let list = self.list.get_or_insert_with(|| {
            h.object_inventory(&object_id)
                .iter()
                .map(|o| o.id)
                .collect()
        });
        let index = list
            .iter()
            .position(|i| *i == item.id)
            .map_or(-1, |i| i as i32);
        h.flag(q::FILTER_CHECK, number(index))
    }
}

/// # Safety
/// `host` is valid; ids are 16 bytes or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_r4b_world_to_agent(
    host: *const VksInvR4bHost,
    object_id: *const u8,
    category_id: *const u8,
    drop: bool,
    filter_types: i64,
) -> bool {
    unsafe {
        with_host(host, false, |h| {
            let object = uuid_arg(object_id);
            let category = uuid_arg(category_id);
            let mut filter = HostFilter {
                raw: h.raw,
                object_id: object,
                types: filter_types as u64,
                list: None,
            };
            let f: Option<&mut dyn ItemFilter> = if filter_types < 0 {
                None
            } else {
                Some(&mut filter)
            };
            give::world_to_agent(h, &object, &category, drop, f)
        })
    }
}

/// # Safety
/// `host` is valid; ids are 16 bytes or null; `item`/`cat` valid or null;
/// `dest` holds `dest_len` bytes; `accept` writable or null.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn vks_inv_r4b_give_drag_and_drop(
    host: *const VksInvR4bHost,
    dest_agent: *const u8,
    session: *const u8,
    drop: bool,
    cargo_type: i32,
    item: *const VksInvR4bItem,
    cat: *const VksInvR4bCat,
    dest: *const u8,
    dest_len: usize,
    accept: *mut i32,
) -> bool {
    unsafe {
        let (r, a) = with_host(host, (false, acceptance(Acceptance::No)), |h| {
            let cargo = match (opt_item(item), opt_cat(cat)) {
                (Some(i), _) => Cargo::Item(i),
                (None, Some(c)) => Cargo::Category(c),
                (None, None) => {
                    set_error("vks_inv_r4b_give_drag_and_drop: no cargo");
                    return (false, acceptance(Acceptance::No));
                }
            };
            let d = sd_arg(dest, dest_len);
            let (r, a) = give::give_drag_and_drop(
                h,
                &uuid_arg(dest_agent),
                &uuid_arg(session),
                drop,
                cargo_type,
                &cargo,
                &d,
            );
            (r, acceptance(a))
        });
        if let Some(out) = accept.as_mut() {
            *out = a;
        }
        r
    }
}

/// # Safety
/// `host` is valid; `item`/`cat` valid or null; `obj` 16 bytes or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_r4b_dad3d_give(
    host: *const VksInvR4bHost,
    kind: i32,
    source: i32,
    item: *const VksInvR4bItem,
    cat: *const VksInvR4bCat,
    obj: *const u8,
    drop: bool,
) -> i32 {
    unsafe {
        with_host(host, acceptance(Acceptance::No), |h| {
            let source = match source {
                0 => DragSource::Agent,
                1 => DragSource::World,
                2 => DragSource::Notecard,
                3 => DragSource::Library,
                4 => DragSource::Viewer,
                _ => DragSource::People,
            };
            let tool = DragState {
                source,
                item: opt_item(item),
                cat: opt_cat(cat),
            };
            let o = (!obj.is_null()).then(|| uuid_arg(obj));
            acceptance(give::dad3d_give(h, kind, &tool, o.as_ref(), drop))
        })
    }
}

/// # Safety
/// `host` is valid; ids are 16 bytes or null; the strings hold their
/// lengths.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn vks_inv_r4b_share_confirmation(
    host: *const VksInvR4bHost,
    name: *const u8,
    name_len: usize,
    item_id: *const u8,
    item_name: *const u8,
    item_name_len: usize,
    dest: *const u8,
    dest_len: usize,
    dest_agent: *const u8,
    session: *const u8,
) {
    unsafe {
        with_host(host, (), |h| {
            give::show_object_sharing_confirmation(
                h,
                bytes(name, name_len),
                &uuid_arg(item_id),
                bytes(item_name, item_name_len),
                bytes(dest, dest_len),
                &uuid_arg(dest_agent),
                &uuid_arg(session),
            );
        })
    }
}
