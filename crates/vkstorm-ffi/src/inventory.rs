//! Rung seven, sub-rung R1: the inventory data model via `vkstorm-inventory`
//! (`vks_inv_*`): `LLPermissions`, `LLAggregatePermissions`, `LLSaleInfo`,
//! the item, category and object records with their CRCs, name and
//! description rules, legacy text and message block forms, and
//! `LLNotecard`'s text.
//!
//! The `LLSD` forms of these classes (`asLLSD`, `fromLLSD`, the category
//! cache import and export, the permission and sale `LLSD` messages) stay
//! C++ in the viewer until R2 and R3a stop building C++ `LLSD` trees:
//! flattening a C++ tree across this ABI cost more than the rules saved.
//! The crate keeps their logic for R2 and R3a.
//!
//! How records cross:
//!
//! - `LLPermissions`, `LLSaleInfo` and `LLAggregatePermissions` have no
//!   virtual functions, and the crate's records have their memory layout
//!   ([`VksInvPerms`], [`VksInvSale`], [`VksInvAggregate`] are those records).
//!   C++ passes `this`, cast, with no copy; the calls read and write it in
//!   place.
//! - Items, categories and objects are virtual C++ classes, so C++ fills a
//!   flat view ([`VksInvItem`], [`VksInvCategory`], [`VksInvObject`]) whose
//!   strings borrow its own `std::string`s for the length of the call.
//!   Results come back in a [`VksInvRecords`] block owned by Rust: flat
//!   records of the same types whose strings point into the block, a text
//!   buffer and the call's scalar results, read through one
//!   [`VksInvRecordsView`] and freed with `vks_inv_records_free`.
//! - Legacy text readers take the rest of the C++ stream as one window and
//!   report the bytes consumed and the stream state ([`VksInvStream`]).
//! - Message blocks cross as wire records ([`VksInvItemBlock`],
//!   [`VksInvFolderBlock`]): C++ keeps the `add*Fast`/`get*Fast` field I/O,
//!   the crate makes the decisions (CRC, verdict, repairs).
//!
//! Every call catches panics at the boundary: it returns null, false or 0
//! (or does nothing) and leaves the text in `vks_inv_last_error`.
//!
//! In-place calls. The permission, sale and aggregate calls on the in-place
//! records are the hot ones (`allowOperationBy` and friends). They neither
//! allocate nor index, so nothing in them can panic; their guard costs
//! nothing (the release build of `vks_inv_perms_allow_operation_by` is the
//! same machine code with and without it, 66 instructions on x86-64), so
//! they keep it like every other call. Their record pointers must be valid.
//! UUID arguments point to 16 bytes (`LLUUID::mData`) and are copied before
//! the record is written, so they may point into it; a null UUID pointer
//! reads as the null UUID. A record argument that may alias the record
//! written (`accumulate(*this)`) is copied first as well.

use std::cell::RefCell;
use std::ffi::{CString, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::{ptr, slice};

use vkstorm_inventory::aggregate::AggregatePermissions;
use vkstorm_inventory::category::{Category, ViewerCategory};
use vkstorm_inventory::item::Item;
use vkstorm_inventory::legacy::notecard::Notecard;
use vkstorm_inventory::legacy::reader::Stream;
use vkstorm_inventory::message::{
    FolderBlock, ItemBlock, apply_folder_block, apply_item_block, apply_sale_fields,
};
use vkstorm_inventory::object::Object;
use vkstorm_inventory::perms::{Permissions, Uuid, mask_to_string};
use vkstorm_inventory::sale::SaleInfo;
use vkstorm_inventory::text;
use vkstorm_inventory::types::{AssetType, FolderType, InventoryType};

// ---------------------------------------------------------------------------
// Errors and argument helpers

thread_local! {
    static LAST_ERROR: RefCell<CString> = RefCell::new(CString::default());
}

fn set_error(message: impl Into<Vec<u8>>) {
    let c = CString::new(message).unwrap_or_default();
    LAST_ERROR.with(|e| *e.borrow_mut() = c);
}

/// The text of the last failure on this thread, NUL-terminated, valid until
/// the next failing `vks_inv_*` call on the thread.
#[unsafe(no_mangle)]
pub extern "C" fn vks_inv_last_error() -> *const c_char {
    LAST_ERROR.with(|e| e.borrow().as_ptr())
}

/// Runs `f`, turning a panic into `fail` with the error text set.
pub(crate) fn guard<T>(fail: T, f: impl FnOnce() -> T) -> T {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(v) => v,
        Err(_) => {
            set_error("internal failure in vkstorm-inventory");
            fail
        }
    }
}

pub(crate) unsafe fn bytes<'a>(p: *const u8, len: usize) -> &'a [u8] {
    if p.is_null() || len == 0 {
        &[]
    } else {
        unsafe { slice::from_raw_parts(p, len) }
    }
}

/// A UUID argument (16 bytes, `LLUUID::mData`), copied so that it may alias
/// a record the call writes; null reads as the null UUID.
pub(crate) unsafe fn uuid_arg(p: *const u8) -> Uuid {
    if p.is_null() {
        [0; 16]
    } else {
        unsafe { ptr::read(p as *const Uuid) }
    }
}

/// Copies `s` into the caller's buffer (NUL-terminated when there is room)
/// and returns the full length, as `vks_ffi_string` expects.
unsafe fn copy_out(s: &[u8], buf: *mut c_char, cap: usize) -> usize {
    if !buf.is_null() && cap > 0 {
        let n = s.len().min(cap - 1);
        unsafe {
            ptr::copy_nonoverlapping(s.as_ptr(), buf as *mut u8, n);
            *buf.add(n) = 0;
        }
    }
    s.len()
}

// ---------------------------------------------------------------------------
// Records

/// `LLPermissions`, in place: the crate's record has the C++ layout (88
/// bytes, alignment 4; see `Permissions`).
pub type VksInvPerms = Permissions;
/// `LLSaleInfo`, in place (8 bytes, alignment 4).
pub type VksInvSale = SaleInfo;
/// `LLAggregatePermissions`, in place (3 bytes).
pub type VksInvAggregate = AggregatePermissions;

/// Bytes: borrowed from C++ for the length of a call on the way in, owned
/// by the result block on the way out. `ptr` may be anything when `len` is
/// 0.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvBytes {
    pub ptr: *const u8,
    pub len: usize,
}

impl VksInvBytes {
    pub(crate) fn of(b: &[u8]) -> VksInvBytes {
        VksInvBytes {
            ptr: b.as_ptr(),
            len: b.len(),
        }
    }

    pub(crate) unsafe fn get<'a>(&self) -> &'a [u8] {
        unsafe { bytes(self.ptr, self.len) }
    }
}

/// The `LLInventoryObject` fields.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvObject {
    pub id: Uuid,
    pub parent: Uuid,
    pub thumbnail: Uuid,
    /// `LLAssetType::EType`, any value.
    pub asset_type: i32,
    pub favorite: bool,
    /// `time_t`.
    pub creation_date: i64,
    pub name: VksInvBytes,
    /// The script runtime (`mRuntime`), empty for none.
    pub runtime: VksInvBytes,
}

/// The `LLInventoryItem` fields.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvItem {
    pub object: VksInvObject,
    pub perms: VksInvPerms,
    pub asset: Uuid,
    pub sale: VksInvSale,
    /// `LLInventoryType::EType`, any value.
    pub inv_type: i32,
    pub flags: u32,
    pub description: VksInvBytes,
}

/// The `LLInventoryCategory` fields, and the `LLViewerInventoryCategory`
/// owner and version its cache LLSD carries (no call reads them since the
/// LLSD forms stayed C++; they are carried through unchanged).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvCategory {
    pub object: VksInvObject,
    /// `LLFolderType::EType`, any value.
    pub preferred_type: i32,
    pub owner: Uuid,
    pub version: i32,
}

/// `std::ios_base::iostate` bits of [`VksInvStream`].
pub const VKS_INV_STREAM_EOF: u32 = 1;
pub const VKS_INV_STREAM_FAIL: u32 = 2;
pub const VKS_INV_STREAM_BAD: u32 = 4;

/// A legacy reader's stream: on entry `state` holds the C++ stream's state
/// bits; on return `consumed` is the bytes the reader took from the window
/// and `state` the stream's state afterwards (the entry bits included).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VksInvStream {
    pub consumed: usize,
    pub state: u32,
}

/// An item block as it travels (the `ItemData` layout): every value in its
/// wire type. `asset` is the `AssetID` slot, or `TransactionID` for
/// `LLViewerInventoryItem::packMessage`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvItemBlock {
    pub item_id: Uuid,
    pub folder_id: Uuid,
    pub creator_id: Uuid,
    pub owner_id: Uuid,
    pub group_id: Uuid,
    pub base_mask: u32,
    pub owner_mask: u32,
    pub group_mask: u32,
    pub everyone_mask: u32,
    pub next_owner_mask: u32,
    pub group_owned: bool,
    pub asset_type: i8,
    pub inv_type: i8,
    pub sale_type: u8,
    pub asset: Uuid,
    pub flags: u32,
    pub sale_price: i32,
    pub creation_date: i32,
    pub crc: u32,
    pub name: VksInvBytes,
    pub description: VksInvBytes,
}

/// A folder block as it travels (`FolderData` and the like).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VksInvFolderBlock {
    pub folder_id: Uuid,
    pub parent_id: Uuid,
    pub preferred_type: i8,
    pub name: VksInvBytes,
}

/// What a call returned in a [`VksInvRecords`] block. Every pointer points
/// into the block and lives until `vks_inv_records_free`.
#[repr(C)]
pub struct VksInvRecordsView {
    pub items: *const VksInvItem,
    pub item_count: usize,
    pub categories: *const VksInvCategory,
    pub category_count: usize,
    pub objects: *const VksInvObject,
    pub object_count: usize,
    /// Text: a legacy export, or a notecard's text.
    pub text: VksInvBytes,
    /// The C++ function's `bool` result (success, CRC verdict).
    pub ok: bool,
    /// The legacy readers' stream.
    pub stream: VksInvStream,
    /// A notecard's version and embedded-items version.
    pub version: i32,
    pub embedded_version: i32,
}

// Layout: checked here and, through the same numbers, by static asserts in
// the header (64-bit targets, the only ones the viewer builds).
const _: () = {
    use std::mem::{offset_of, size_of};
    assert!(
        size_of::<VksInvPerms>() == 88
            && size_of::<VksInvSale>() == 8
            && size_of::<VksInvAggregate>() == 3
    );
    if size_of::<usize>() == 8 {
        assert!(size_of::<VksInvBytes>() == 16);
        assert!(size_of::<VksInvObject>() == 96 && offset_of!(VksInvObject, creation_date) == 56);
        assert!(size_of::<VksInvItem>() == 232 && offset_of!(VksInvItem, perms) == 96);
        assert!(offset_of!(VksInvItem, sale) == 200 && offset_of!(VksInvItem, description) == 216);
        assert!(size_of::<VksInvCategory>() == 120);
        assert!(size_of::<VksInvStream>() == 16);
        assert!(size_of::<VksInvItemBlock>() == 168 && offset_of!(VksInvItemBlock, asset) == 104);
        assert!(size_of::<VksInvFolderBlock>() == 56);
    }
};

unsafe fn object_from(v: &VksInvObject) -> Object {
    Object {
        id: v.id,
        parent: v.parent,
        thumbnail: v.thumbnail,
        asset_type: AssetType(v.asset_type),
        favorite: v.favorite,
        creation_date: v.creation_date,
        runtime: unsafe { v.runtime.get() }.to_vec(),
        name: unsafe { v.name.get() }.to_vec(),
    }
}

pub(crate) fn object_view(o: &Object) -> VksInvObject {
    VksInvObject {
        id: o.id,
        parent: o.parent,
        thumbnail: o.thumbnail,
        asset_type: o.asset_type.0,
        favorite: o.favorite,
        creation_date: o.creation_date,
        name: VksInvBytes::of(&o.name),
        runtime: VksInvBytes::of(&o.runtime),
    }
}

pub(crate) unsafe fn item_from(v: &VksInvItem) -> Item {
    Item {
        object: unsafe { object_from(&v.object) },
        description: unsafe { v.description.get() }.to_vec(),
        ..item_scalars(v)
    }
}

/// The item without its strings (name, description, runtime empty), for the
/// rules the crate documents as not reading them: `crc32`,
/// `accumulate_permission_slam_bits` and the block's scalars. Empty vectors
/// do not allocate.
fn item_scalars(v: &VksInvItem) -> Item {
    let o = &v.object;
    Item {
        object: Object {
            id: o.id,
            parent: o.parent,
            thumbnail: o.thumbnail,
            asset_type: AssetType(o.asset_type),
            favorite: o.favorite,
            creation_date: o.creation_date,
            runtime: Vec::new(),
            name: Vec::new(),
        },
        permissions: v.perms,
        asset: v.asset,
        description: Vec::new(),
        sale_info: v.sale,
        inv_type: InventoryType(v.inv_type),
        flags: v.flags,
    }
}

pub(crate) fn item_view(i: &Item) -> VksInvItem {
    VksInvItem {
        object: object_view(&i.object),
        perms: i.permissions,
        asset: i.asset,
        sale: i.sale_info,
        inv_type: i.inv_type.0,
        flags: i.flags,
        description: VksInvBytes::of(&i.description),
    }
}

unsafe fn category_from(v: &VksInvCategory) -> ViewerCategory {
    ViewerCategory {
        category: Category {
            object: unsafe { object_from(&v.object) },
            preferred_type: FolderType(v.preferred_type),
        },
        owner: v.owner,
        version: v.version,
    }
}

fn category_view(c: &ViewerCategory) -> VksInvCategory {
    VksInvCategory {
        object: object_view(&c.category.object),
        preferred_type: c.category.preferred_type.0,
        owner: c.owner,
        version: c.version,
    }
}

/// A result block: the records and text a call returns, and their view.
pub struct VksInvRecords {
    items: Vec<Item>,
    categories: Vec<ViewerCategory>,
    objects: Vec<Object>,
    text: Vec<u8>,
    item_views: Vec<VksInvItem>,
    category_views: Vec<VksInvCategory>,
    object_views: Vec<VksInvObject>,
    view: VksInvRecordsView,
}

impl VksInvRecords {
    fn new(ok: bool) -> VksInvRecords {
        VksInvRecords {
            items: Vec::new(),
            categories: Vec::new(),
            objects: Vec::new(),
            text: Vec::new(),
            item_views: Vec::new(),
            category_views: Vec::new(),
            object_views: Vec::new(),
            view: VksInvRecordsView {
                items: ptr::null(),
                item_count: 0,
                categories: ptr::null(),
                category_count: 0,
                objects: ptr::null(),
                object_count: 0,
                text: VksInvBytes {
                    ptr: ptr::null(),
                    len: 0,
                },
                ok,
                stream: VksInvStream::default(),
                version: 0,
                embedded_version: 0,
            },
        }
    }

    fn text(text: Vec<u8>) -> VksInvRecords {
        VksInvRecords {
            text,
            ..VksInvRecords::new(true)
        }
    }

    fn item(ok: bool, item: Item) -> VksInvRecords {
        VksInvRecords {
            items: vec![item],
            ..VksInvRecords::new(ok)
        }
    }

    fn category(ok: bool, category: ViewerCategory) -> VksInvRecords {
        VksInvRecords {
            categories: vec![category],
            ..VksInvRecords::new(ok)
        }
    }

    fn with_stream(mut self, s: &Stream<'_>) -> VksInvRecords {
        self.view.stream = stream_result(s);
        self
    }

    /// Builds the views (whose pointers stay valid: the strings and records
    /// live on the heap and are not touched again) and hands the block out.
    fn into_raw(mut self) -> *mut VksInvRecords {
        self.item_views = self.items.iter().map(item_view).collect();
        self.category_views = self.categories.iter().map(category_view).collect();
        self.object_views = self.objects.iter().map(object_view).collect();
        let v = &mut self.view;
        v.items = self.item_views.as_ptr();
        v.item_count = self.item_views.len();
        v.categories = self.category_views.as_ptr();
        v.category_count = self.category_views.len();
        v.objects = self.object_views.as_ptr();
        v.object_count = self.object_views.len();
        v.text = VksInvBytes::of(&self.text);
        Box::into_raw(Box::new(self))
    }
}

/// The view of a result block.
///
/// # Safety
/// `r` must come from a `vks_inv_*` call and be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_records_view(r: *const VksInvRecords) -> *const VksInvRecordsView {
    if r.is_null() {
        ptr::null()
    } else {
        unsafe { &(*r).view }
    }
}

/// Releases a result block. Null is ignored.
///
/// # Safety
/// `r` must come from a `vks_inv_*` call and not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_records_free(r: *mut VksInvRecords) {
    if !r.is_null() {
        drop(unsafe { Box::from_raw(r) });
    }
}

/// A result block from `f`, or null when it panics.
fn records(f: impl FnOnce() -> Option<VksInvRecords>) -> *mut VksInvRecords {
    guard(ptr::null_mut(), || {
        f().map_or(ptr::null_mut(), VksInvRecords::into_raw)
    })
}

/// A stream over the window in the C++ stream's entry state.
unsafe fn stream_in<'a>(data: *const u8, len: usize, stream: *const VksInvStream) -> Stream<'a> {
    let state = if stream.is_null() {
        0
    } else {
        unsafe { (*stream).state }
    };
    Stream::with_state(
        unsafe { bytes(data, len) },
        state & VKS_INV_STREAM_EOF != 0,
        state & VKS_INV_STREAM_FAIL != 0,
        state & VKS_INV_STREAM_BAD != 0,
    )
}

fn stream_result(s: &Stream<'_>) -> VksInvStream {
    let (eof, fail, bad) = s.state_bits();
    let mut state = 0;
    if eof {
        state |= VKS_INV_STREAM_EOF;
    }
    if fail {
        state |= VKS_INV_STREAM_FAIL;
    }
    if bad {
        state |= VKS_INV_STREAM_BAD;
    }
    VksInvStream {
        consumed: s.pos(),
        state,
    }
}

// ---------------------------------------------------------------------------
// LLPermissions: the in-place calls

/// `LLPermissions::init(creator, owner, last_owner, group)`.
///
/// # Safety
/// An in-place call: see the module documentation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_perms_init(
    p: *mut VksInvPerms,
    creator: *const u8,
    owner: *const u8,
    last_owner: *const u8,
    group: *const u8,
) {
    guard((), || {
        let (c, o, l, g) = unsafe {
            (
                uuid_arg(creator),
                uuid_arg(owner),
                uuid_arg(last_owner),
                uuid_arg(group),
            )
        };
        unsafe { (*p).init(&c, &o, &l, &g) }
    })
}

/// `LLPermissions::initMasks(base, owner, everyone, group, next)`, in the
/// C++ argument order.
///
/// # Safety
/// An in-place call: see the module documentation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_perms_init_masks(
    p: *mut VksInvPerms,
    base: u32,
    owner: u32,
    everyone: u32,
    group: u32,
    next: u32,
) {
    guard((), || unsafe {
        (*p).init_masks(base, owner, everyone, group, next)
    })
}

/// `LLPermissions::initMasks(LLInventoryType::EType)`.
///
/// # Safety
/// An in-place call: see the module documentation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_perms_init_masks_for_type(p: *mut VksInvPerms, inv_type: i32) {
    guard((), || unsafe {
        (*p).init_masks_for_type(InventoryType(inv_type))
    })
}

/// `LLPermissions::getOwnership`: true with the owner (or group) and the
/// group flag written; false with both left as they are.
///
/// # Safety
/// An in-place call (see the module documentation); `owner_out` must be writable for 16 bytes and
/// `is_group_owned` valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_perms_get_ownership(
    p: *const VksInvPerms,
    owner_out: *mut u8,
    is_group_owned: *mut bool,
) -> bool {
    guard(false, || match unsafe { (*p).ownership() } {
        Some((id, group)) => {
            unsafe {
                ptr::write(owner_out as *mut Uuid, id);
                *is_group_owned = group;
            }
            true
        }
        None => false,
    })
}

/// `LLPermissions::getSafeOwner` of an owned record: true with the owner
/// (or group) written. False when unowned, where the C++ logs a warning and
/// returns a random UUID: that stays with the caller.
///
/// # Safety
/// An in-place call (see the module documentation); `out` must be writable for 16 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_perms_safe_owner(p: *const VksInvPerms, out: *mut u8) -> bool {
    guard(false, || match unsafe { (*p).safe_owner() } {
        Some(id) => {
            unsafe { ptr::write(out as *mut Uuid, id) };
            true
        }
        None => false,
    })
}

/// `LLPermissions::getCRC32`.
///
/// # Safety
/// An in-place call: see the module documentation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_perms_crc32(p: *const VksInvPerms) -> u32 {
    guard(0, || unsafe { (*p).crc32() })
}

/// `LLPermissions::fix`.
///
/// # Safety
/// An in-place call: see the module documentation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_perms_fix(p: *mut VksInvPerms) {
    guard((), || unsafe { (*p).fix() })
}

/// `LLPermissions::fixFairUse`.
///
/// # Safety
/// An in-place call: see the module documentation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_perms_fix_fair_use(p: *mut VksInvPerms) {
    guard((), || unsafe { (*p).fix_fair_use() })
}

/// `LLPermissions::fixOwnership`.
///
/// # Safety
/// An in-place call: see the module documentation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_perms_fix_ownership(p: *mut VksInvPerms) {
    guard((), || unsafe { (*p).fix_ownership() })
}

/// `LLPermissions::accumulate(perm)`; `perm` may be `p`.
///
/// # Safety
/// An in-place call: see the module documentation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_perms_accumulate(p: *mut VksInvPerms, perm: *const VksInvPerms) {
    guard((), || {
        let other = unsafe { ptr::read(perm) };
        unsafe { (*p).accumulate(&other) }
    })
}

/// `LLPermissions::setOwnerAndGroup`.
///
/// # Safety
/// An in-place call: see the module documentation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_perms_set_owner_and_group(
    p: *mut VksInvPerms,
    agent: *const u8,
    owner: *const u8,
    group: *const u8,
    is_atomic: bool,
) -> bool {
    guard(false, || {
        let (a, o, g) = unsafe { (uuid_arg(agent), uuid_arg(owner), uuid_arg(group)) };
        unsafe { (*p).set_owner_and_group(&a, &o, &g, is_atomic) }
    })
}

/// `LLPermissions::setLastOwner`.
///
/// # Safety
/// An in-place call: see the module documentation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_perms_set_last_owner(p: *mut VksInvPerms, last_owner: *const u8) {
    guard((), || {
        let l = unsafe { uuid_arg(last_owner) };
        unsafe { (*p).set_last_owner(&l) }
    })
}

/// `LLPermissions::yesReallySetOwner`.
///
/// # Safety
/// An in-place call: see the module documentation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_perms_yes_really_set_owner(
    p: *mut VksInvPerms,
    owner: *const u8,
    group_owned: bool,
) {
    guard((), || {
        let o = unsafe { uuid_arg(owner) };
        unsafe { (*p).yes_really_set_owner(&o, group_owned) }
    })
}

/// `LLPermissions::deedToGroup`.
///
/// # Safety
/// An in-place call: see the module documentation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_perms_deed_to_group(
    p: *mut VksInvPerms,
    agent: *const u8,
    group: *const u8,
) -> bool {
    guard(false, || {
        let (a, g) = unsafe { (uuid_arg(agent), uuid_arg(group)) };
        unsafe { (*p).deed_to_group(&a, &g) }
    })
}

/// `LLPermissions::setBaseBits`.
///
/// # Safety
/// An in-place call: see the module documentation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_perms_set_base_bits(
    p: *mut VksInvPerms,
    agent: *const u8,
    set: bool,
    bits: u32,
) -> bool {
    guard(false, || {
        let a = unsafe { uuid_arg(agent) };
        unsafe { (*p).set_base_bits(&a, set, bits) }
    })
}

/// `LLPermissions::setOwnerBits`.
///
/// # Safety
/// An in-place call: see the module documentation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_perms_set_owner_bits(
    p: *mut VksInvPerms,
    agent: *const u8,
    set: bool,
    bits: u32,
) -> bool {
    guard(false, || {
        let a = unsafe { uuid_arg(agent) };
        unsafe { (*p).set_owner_bits(&a, set, bits) }
    })
}

macro_rules! shared_bits_fn {
    ($name:ident, $method:ident, $cpp:literal) => {
        #[doc = concat!("`LLPermissions::", $cpp, "`.")]
        ///
        /// # Safety
        /// An in-place call: see the module documentation.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(
            p: *mut VksInvPerms,
            agent: *const u8,
            group: *const u8,
            set: bool,
            bits: u32,
        ) -> bool {
            guard(false, || {
                let (a, g) = unsafe { (uuid_arg(agent), uuid_arg(group)) };
                unsafe { (*p).$method(&a, &g, set, bits) }
            })
        }
    };
}

shared_bits_fn!(vks_inv_perms_set_group_bits, set_group_bits, "setGroupBits");
shared_bits_fn!(
    vks_inv_perms_set_everyone_bits,
    set_everyone_bits,
    "setEveryoneBits"
);
shared_bits_fn!(
    vks_inv_perms_set_next_owner_bits,
    set_next_owner_bits,
    "setNextOwnerBits"
);

/// `LLPermissions::allowOperationBy(op, requester, group)`; the inline
/// `allowModifyBy`, `allowCopyBy` and `allowMoveBy` are this call with
/// `PERM_MODIFY`, `PERM_COPY` and `PERM_MOVE` (a null `group` for their
/// one-argument forms).
///
/// # Safety
/// An in-place call: see the module documentation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_perms_allow_operation_by(
    p: *const VksInvPerms,
    op: u32,
    requester: *const u8,
    group: *const u8,
) -> bool {
    guard(false, || {
        let (r, g) = unsafe { (uuid_arg(requester), uuid_arg(group)) };
        unsafe { (*p).allow_operation_by(op, &r, &g) }
    })
}

/// `LLPermissions::allowTransferTo`.
///
/// # Safety
/// An in-place call: see the module documentation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_perms_allow_transfer_to(
    p: *const VksInvPerms,
    agent: *const u8,
) -> bool {
    guard(false, || {
        let a = unsafe { uuid_arg(agent) };
        unsafe { (*p).allow_transfer_to(&a) }
    })
}

/// `LLPermissions::allowExportBy`.
///
/// # Safety
/// An in-place call: see the module documentation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_perms_allow_export_by(
    p: *const VksInvPerms,
    agent: *const u8,
) -> bool {
    guard(false, || {
        let a = unsafe { uuid_arg(agent) };
        unsafe { (*p).allow_export_by(&a) }
    })
}

/// `LLPermissions::allowOpenSimExportBy` (OpenSim builds only).
///
/// # Safety
/// An in-place call: see the module documentation.
#[cfg(feature = "opensim")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_perms_allow_open_sim_export_by(
    p: *const VksInvPerms,
    agent: *const u8,
) -> bool {
    guard(false, || {
        let a = unsafe { uuid_arg(agent) };
        unsafe { (*p).allow_open_sim_export_by(&a) }
    })
}

/// `LLPermissions::operator==`: every field.
///
/// # Safety
/// An in-place call: see the module documentation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_perms_equal(a: *const VksInvPerms, b: *const VksInvPerms) -> bool {
    guard(false, || unsafe { *a == *b })
}

/// `mask_to_string(mask, isOpenSim)` into `(buf, cap)` (see
/// `vks_ffi_string`); 4 characters, or 5 with the export column in an
/// OpenSim build when `is_open_sim`. A buffer of 6 bytes always suffices.
///
/// # Safety
/// `buf` must be writable for `cap` bytes unless null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_mask_to_string(
    mask: u32,
    is_open_sim: bool,
    buf: *mut c_char,
    cap: usize,
) -> usize {
    guard(0, || unsafe {
        copy_out(mask_to_string(mask, is_open_sim).as_bytes(), buf, cap)
    })
}

// ---------------------------------------------------------------------------
// LLPermissions: legacy text

/// `LLPermissions::importLegacyStream` into `p`, over the rest of the C++
/// stream (`data`, `len`; the `permissions 0` line already read). Fills
/// `stream` (see [`VksInvStream`]) and returns the C++ result (true); false
/// only on a failure, with `p` and `stream` unchanged.
///
/// # Safety
/// `p` and `stream` must be valid; `data` readable for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_perms_import_legacy(
    p: *mut VksInvPerms,
    data: *const u8,
    len: usize,
    stream: *mut VksInvStream,
) -> bool {
    guard(false, || {
        let mut s = unsafe { stream_in(data, len, stream) };
        let mut perms = unsafe { *p };
        let ok = perms.import_legacy_stream(&mut s);
        unsafe {
            *p = perms;
            *stream = stream_result(&s);
        }
        ok
    })
}

/// `LLPermissions::exportLegacyStream`: the text.
///
/// # Safety
/// `p` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_perms_export_legacy(p: *const VksInvPerms) -> *mut VksInvRecords {
    records(|| {
        let mut out = Vec::with_capacity(512);
        unsafe { (*p).export_legacy_stream(&mut out) };
        Some(VksInvRecords::text(out))
    })
}

// ---------------------------------------------------------------------------
// LLAggregatePermissions (in place)

/// `LLAggregatePermissions::getValue(bit)`.
///
/// # Safety
/// An in-place call: see the module documentation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_aggregate_value(a: *const VksInvAggregate, bit: u32) -> u8 {
    guard(0, || unsafe { (*a).value(bit) })
}

/// `LLAggregatePermissions::getU8`, as `packMessage` sends it.
///
/// # Safety
/// An in-place call: see the module documentation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_aggregate_to_u8(a: *const VksInvAggregate) -> u8 {
    guard(0, || unsafe { (*a).to_u8() })
}

/// The state `LLAggregatePermissions::unpackMessage` reads from its byte.
///
/// # Safety
/// An in-place call: see the module documentation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_aggregate_from_u8(a: *mut VksInvAggregate, byte: u8) {
    guard((), || unsafe { *a = AggregatePermissions::from_u8(byte) })
}

/// `LLAggregatePermissions::isEmpty`.
///
/// # Safety
/// An in-place call: see the module documentation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_aggregate_is_empty(a: *const VksInvAggregate) -> bool {
    guard(false, || unsafe { (*a).is_empty() })
}

/// `LLAggregatePermissions::aggregate(PermissionMask)`.
///
/// # Safety
/// An in-place call: see the module documentation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_aggregate_mask(a: *mut VksInvAggregate, mask: u32) {
    guard((), || unsafe { (*a).aggregate_mask(mask) })
}

/// `LLAggregatePermissions::aggregate(const LLAggregatePermissions&)`;
/// `other` may be `a`.
///
/// # Safety
/// An in-place call: see the module documentation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_aggregate_merge(
    a: *mut VksInvAggregate,
    other: *const VksInvAggregate,
) {
    guard((), || {
        let o = unsafe { ptr::read(other) };
        unsafe { (*a).aggregate(&o) }
    })
}

// ---------------------------------------------------------------------------
// LLSaleInfo

/// `LLSaleInfo::getCRC32`.
///
/// # Safety
/// An in-place call: see the module documentation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_sale_crc32(s: *const VksInvSale) -> u32 {
    guard(0, || unsafe { (*s).crc32() })
}

/// `LLSaleInfo::setSalePrice` (clamped to `0..=S32_MAX`).
///
/// # Safety
/// An in-place call: see the module documentation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_sale_set_price(s: *mut VksInvSale, price: i32) {
    guard((), || unsafe { (*s).set_sale_price(price) })
}

/// `LLSaleInfo::accumulate`; `other` may be `s`.
///
/// # Safety
/// An in-place call: see the module documentation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_sale_accumulate(s: *mut VksInvSale, other: *const VksInvSale) {
    guard((), || {
        let o = unsafe { ptr::read(other) };
        unsafe { (*s).accumulate(&o) }
    })
}

/// `LLSaleInfo::unpackMessage`/`unpackMultiMessage` from the read
/// `SaleType` (`U8`) and `SalePrice` (`S32`) values.
///
/// # Safety
/// An in-place call: see the module documentation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_sale_unpack_wire(
    s: *mut VksInvSale,
    sale_type: u8,
    sale_price: i32,
) {
    guard((), || unsafe {
        apply_sale_fields(sale_type, sale_price, &mut *s)
    })
}

/// `LLSaleInfo::importLegacyStream(input, has_perm_mask, perm_mask)` into
/// `s`, over the rest of the C++ stream (the `sale_info\t0` line already
/// read). Fills `stream`; returns the C++ result (true); false only on a
/// failure, with everything unchanged.
///
/// # Safety
/// Every pointer must be valid; `data` readable for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_sale_import_legacy(
    s: *mut VksInvSale,
    data: *const u8,
    len: usize,
    has_perm_mask: *mut bool,
    perm_mask: *mut u32,
    stream: *mut VksInvStream,
) -> bool {
    guard(false, || {
        let mut st = unsafe { stream_in(data, len, stream) };
        let (mut sale, mut has, mut mask) = unsafe { (*s, *has_perm_mask, *perm_mask) };
        let ok = sale.import_legacy_stream(&mut st, &mut has, &mut mask);
        unsafe {
            *s = sale;
            *has_perm_mask = has;
            *perm_mask = mask;
            *stream = stream_result(&st);
        }
        ok
    })
}

/// `LLSaleInfo::exportLegacyStream`: the text.
///
/// # Safety
/// `s` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_sale_export_legacy(s: *const VksInvSale) -> *mut VksInvRecords {
    records(|| {
        let mut out = Vec::with_capacity(96);
        unsafe { (*s).export_legacy_stream(&mut out) };
        Some(VksInvRecords::text(out))
    })
}

// ---------------------------------------------------------------------------
// Names and descriptions

/// `DB_INV_ITEM_NAME_STR_LEN + 1`: a buffer this long holds any corrected
/// name and its NUL.
pub const VKS_INV_NAME_BUF: usize = text::DB_INV_ITEM_NAME_STR_LEN + 1;
const _: () = assert!(VKS_INV_NAME_BUF == 64);

/// `LLInventoryObject::correctInventoryName` into `(buf, cap)` (see
/// `vks_ffi_string`); the result is at most 63 bytes.
///
/// # Safety
/// `name` readable for `len` bytes; `buf` writable for `cap` unless null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_correct_name(
    name: *const u8,
    len: usize,
    buf: *mut c_char,
    cap: usize,
) -> usize {
    guard(0, || unsafe {
        copy_out(&text::correct_inventory_name(bytes(name, len)), buf, cap)
    })
}

/// `LLInventoryItem::correctInventoryDescription` into `(buf, cap)`; the
/// result has the input's length.
///
/// # Safety
/// `desc` readable for `len` bytes; `buf` writable for `cap` unless null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_correct_description(
    desc: *const u8,
    len: usize,
    buf: *mut c_char,
    cap: usize,
) -> usize {
    guard(0, || unsafe {
        copy_out(
            &text::correct_inventory_description(bytes(desc, len)),
            buf,
            cap,
        )
    })
}

/// `LLInventoryObject::rename(name)` of an object named `current`: true when
/// the name changes, with the new name in `(buf, cap)` and its length in
/// `out_len` (`VKS_INV_NAME_BUF` bytes always suffice); false when the
/// object keeps its name (or on a failure).
///
/// # Safety
/// The inputs readable for their lengths; `buf` writable for `cap` unless
/// null; `out_len` valid unless null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_rename(
    current: *const u8,
    current_len: usize,
    name: *const u8,
    name_len: usize,
    buf: *mut c_char,
    cap: usize,
    out_len: *mut usize,
) -> bool {
    guard(false, || {
        match text::rename(unsafe { bytes(current, current_len) }, unsafe {
            bytes(name, name_len)
        }) {
            Some(n) => {
                let len = unsafe { copy_out(&n, buf, cap) };
                if !out_len.is_null() {
                    unsafe { *out_len = len };
                }
                true
            }
            None => false,
        }
    })
}

/// `LLInventoryItem::setDescription(desc)` of an item described `current`:
/// true when the description changes, with the new one in `(buf, cap)` and
/// its length (`desc_len`) in `out_len`; false when it stays (or on a
/// failure).
///
/// # Safety
/// As [`vks_inv_rename`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_set_description(
    current: *const u8,
    current_len: usize,
    desc: *const u8,
    desc_len: usize,
    buf: *mut c_char,
    cap: usize,
    out_len: *mut usize,
) -> bool {
    guard(false, || {
        match text::set_description(unsafe { bytes(current, current_len) }, unsafe {
            bytes(desc, desc_len)
        }) {
            Some(d) => {
                let len = unsafe { copy_out(&d, buf, cap) };
                if !out_len.is_null() {
                    unsafe { *out_len = len };
                }
                true
            }
            None => false,
        }
    })
}

// ---------------------------------------------------------------------------
// LLInventoryObject

/// `LLInventoryObject::importLegacyStream` over `in` and the rest of the C++
/// stream (the `inv_object\t0` line already read): one object, `ok` and
/// `stream` set. Null on a failure.
///
/// # Safety
/// `in` must be valid with its strings readable; `data` readable for `len`
/// bytes; `stream`, unless null (a good stream), valid on entry.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_object_import_legacy(
    obj: *const VksInvObject,
    data: *const u8,
    len: usize,
    stream: *const VksInvStream,
) -> *mut VksInvRecords {
    records(|| {
        let mut o = unsafe { object_from(&*obj) };
        let mut s = unsafe { stream_in(data, len, stream) };
        let ok = o.import_legacy_stream(&mut s);
        Some(
            VksInvRecords {
                objects: vec![o],
                ..VksInvRecords::new(ok)
            }
            .with_stream(&s),
        )
    })
}

/// `LLInventoryObject::exportLegacyStream`: the text.
///
/// # Safety
/// `obj` must be valid with its strings readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_object_export_legacy(
    obj: *const VksInvObject,
) -> *mut VksInvRecords {
    records(|| {
        let mut out = Vec::with_capacity(256);
        unsafe { object_from(&*obj) }.export_legacy_stream(&mut out);
        Some(VksInvRecords::text(out))
    })
}

// ---------------------------------------------------------------------------
// LLInventoryItem

/// `LLInventoryItem::getCRC32`. Reads no string; does not allocate.
///
/// # Safety
/// `item` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_item_crc32(item: *const VksInvItem) -> u32 {
    guard(0, || item_scalars(unsafe { &*item }).crc32())
}

/// `LLInventoryItem::accumulatePermissionSlamBits(old_item)`: the item's
/// flags afterwards. Reads no string; does not allocate.
///
/// # Safety
/// `item` and `old_item` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_item_slam_bits(
    item: *const VksInvItem,
    old_item: *const VksInvItem,
) -> u32 {
    let flags = unsafe { (*item).flags };
    guard(flags, || {
        let mut i = item_scalars(unsafe { &*item });
        i.accumulate_permission_slam_bits(&item_scalars(unsafe { &*old_item }));
        i.flags
    })
}

/// The block `LLInventoryItem::packMessage` sends (`slot` null: the asset
/// id) or `LLViewerInventoryItem::packMessage` (`slot`: the transaction
/// id), into `out`; the name and description borrow the item's. Does not
/// allocate. False on a failure.
///
/// # Safety
/// `item` and `out` must be valid; `slot`, unless null, 16 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_item_to_block(
    item: *const VksInvItem,
    slot: *const u8,
    out: *mut VksInvItemBlock,
) -> bool {
    guard(false, || {
        let v = unsafe { &*item };
        let scalars = item_scalars(v);
        // The block's strings are the item's own (`ItemBlock` copies them
        // verbatim), so they are borrowed instead.
        let b = if slot.is_null() {
            ItemBlock::from_item(&scalars)
        } else {
            ItemBlock::from_viewer_item(&scalars, &unsafe { uuid_arg(slot) })
        };
        unsafe {
            *out = VksInvItemBlock {
                item_id: b.item_id,
                folder_id: b.folder_id,
                creator_id: b.creator_id,
                owner_id: b.owner_id,
                group_id: b.group_id,
                base_mask: b.base_mask,
                owner_mask: b.owner_mask,
                group_mask: b.group_mask,
                everyone_mask: b.everyone_mask,
                next_owner_mask: b.next_owner_mask,
                group_owned: b.group_owned,
                asset_type: b.ty,
                inv_type: b.inv_type,
                sale_type: b.sale_type,
                asset: b.asset,
                flags: b.flags,
                sale_price: b.sale_price,
                creation_date: b.creation_date,
                crc: b.crc,
                name: v.object.name,
                description: v.description,
            }
        };
        true
    })
}

/// `LLInventoryItem::unpackMessage` of a read block over `item`: the item
/// afterwards, `ok` the CRC verdict. Null on a failure.
///
/// # Safety
/// `item` and `block` must be valid with their strings readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_item_apply_block(
    item: *const VksInvItem,
    block: *const VksInvItemBlock,
) -> *mut VksInvRecords {
    records(|| {
        let mut i = unsafe { item_from(&*item) };
        let b = unsafe { &*block };
        let wire = ItemBlock {
            item_id: b.item_id,
            folder_id: b.folder_id,
            creator_id: b.creator_id,
            owner_id: b.owner_id,
            group_id: b.group_id,
            base_mask: b.base_mask,
            owner_mask: b.owner_mask,
            group_mask: b.group_mask,
            everyone_mask: b.everyone_mask,
            next_owner_mask: b.next_owner_mask,
            group_owned: b.group_owned,
            asset: b.asset,
            ty: b.asset_type,
            inv_type: b.inv_type,
            flags: b.flags,
            sale_type: b.sale_type,
            sale_price: b.sale_price,
            name: unsafe { b.name.get() }.to_vec(),
            description: unsafe { b.description.get() }.to_vec(),
            creation_date: b.creation_date,
            crc: b.crc,
        };
        let ok = apply_item_block(&wire, &mut i);
        Some(VksInvRecords::item(ok, i))
    })
}

/// `LLInventoryItem::importLegacyStream` over `item` and the rest of the C++
/// stream (the `inv_item\t0` line already read): one item, `ok` and
/// `stream` set. Null on a failure.
///
/// # Safety
/// As [`vks_inv_object_import_legacy`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_item_import_legacy(
    item: *const VksInvItem,
    data: *const u8,
    len: usize,
    stream: *const VksInvStream,
) -> *mut VksInvRecords {
    records(|| {
        let mut i = unsafe { item_from(&*item) };
        let mut s = unsafe { stream_in(data, len, stream) };
        let ok = i.import_legacy_stream(&mut s);
        Some(VksInvRecords::item(ok, i).with_stream(&s))
    })
}

/// `LLInventoryItem::exportLegacyStream(output, include_asset_key)`: the
/// text.
///
/// # Safety
/// `item` must be valid with its strings readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_item_export_legacy(
    item: *const VksInvItem,
    include_asset_key: bool,
) -> *mut VksInvRecords {
    records(|| {
        let mut out = Vec::with_capacity(1024);
        unsafe { item_from(&*item) }.export_legacy_stream(&mut out, include_asset_key);
        Some(VksInvRecords::text(out))
    })
}

// ---------------------------------------------------------------------------
// LLInventoryCategory and LLViewerInventoryCategory

/// `LLInventoryCategory::importLegacyStream` over `cat` and the rest of the
/// C++ stream (the `inv_category\t0` line already read): one category,
/// `ok` and `stream` set. Null on a failure.
///
/// # Safety
/// As [`vks_inv_object_import_legacy`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_category_import_legacy(
    cat: *const VksInvCategory,
    data: *const u8,
    len: usize,
    stream: *const VksInvStream,
) -> *mut VksInvRecords {
    records(|| {
        let mut c = unsafe { category_from(&*cat) };
        let mut s = unsafe { stream_in(data, len, stream) };
        let ok = c.category.import_legacy_stream(&mut s);
        Some(VksInvRecords::category(ok, c).with_stream(&s))
    })
}

/// `LLInventoryCategory::exportLegacyStream`: the text.
///
/// # Safety
/// `cat` must be valid with its strings readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_category_export_legacy(
    cat: *const VksInvCategory,
) -> *mut VksInvRecords {
    records(|| {
        let mut out = Vec::with_capacity(256);
        unsafe { category_from(&*cat) }
            .category
            .export_legacy_stream(&mut out);
        Some(VksInvRecords::text(out))
    })
}

/// `LLInventoryCategory::unpackMessage` of a read folder block over `cat`:
/// one category. Null on a failure.
///
/// # Safety
/// `cat` and `block` must be valid with their strings readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_category_apply_block(
    cat: *const VksInvCategory,
    block: *const VksInvFolderBlock,
) -> *mut VksInvRecords {
    records(|| {
        let mut c = unsafe { category_from(&*cat) };
        let b = unsafe { &*block };
        let wire = FolderBlock {
            folder_id: b.folder_id,
            parent_id: b.parent_id,
            ty: b.preferred_type,
            name: unsafe { b.name.get() }.to_vec(),
        };
        apply_folder_block(&wire, &mut c.category);
        Some(VksInvRecords::category(true, c))
    })
}

// ---------------------------------------------------------------------------
// LLNotecard

/// `LLNotecard::importStream` of a notecard with limit `max_text`, version
/// `version` and embedded version `embedded_version`, over the rest of the
/// C++ stream: the items read (to append to the notecard's, also on
/// failure), `version` and `embedded_version` afterwards, `ok`, `stream`,
/// and on success the text. Null on a failure.
///
/// # Safety
/// `data` readable for `len` bytes; `stream`, unless null (a good stream),
/// valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_notecard_import(
    data: *const u8,
    len: usize,
    max_text: i32,
    version: i32,
    embedded_version: i32,
    stream: *const VksInvStream,
) -> *mut VksInvRecords {
    records(|| {
        let mut n = Notecard {
            version,
            embedded_version,
            ..Notecard::new(max_text)
        };
        let mut s = unsafe { stream_in(data, len, stream) };
        let ok = n.import_stream(&mut s);
        let mut r = VksInvRecords {
            items: n.items,
            text: n.text,
            ..VksInvRecords::new(ok)
        }
        .with_stream(&s);
        r.view.version = n.version;
        r.view.embedded_version = n.embedded_version;
        Some(r)
    })
}

/// `LLNotecard::exportStream` of `count` items and the text: the text.
///
/// # Safety
/// `items` must point to `count` valid items (unless `count` is 0) with
/// their strings readable; `text` readable for `text_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_inv_notecard_export(
    items: *const VksInvItem,
    count: usize,
    text: *const u8,
    text_len: usize,
) -> *mut VksInvRecords {
    records(|| {
        let views: &[VksInvItem] = if items.is_null() || count == 0 {
            &[]
        } else {
            unsafe { slice::from_raw_parts(items, count) }
        };
        let n = Notecard {
            items: views.iter().map(|v| unsafe { item_from(v) }).collect(),
            text: unsafe { bytes(text, text_len) }.to_vec(),
            ..Notecard::default()
        };
        let mut out = Vec::with_capacity(1024 + n.text.len());
        n.export_stream(&mut out);
        Some(VksInvRecords::text(out))
    })
}
