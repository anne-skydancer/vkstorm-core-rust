//! Item flags (`llinventorydefines.h`, the inventory subtype helpers in
//! `llinventory.h`, `LLSettingsType::fromInventoryFlags`) and transaction flags
//! (`lltransactionflags.{h,cpp}`).
//!
//! `LLSettingsType::fromInventoryFlags` is
//! [`SettingsType::from_inventory_flags`](crate::types::SettingsType::from_inventory_flags).

// ---------------------------------------------------------------------------
// llinventorydefines.{h,cpp}
// ---------------------------------------------------------------------------

/// `TASK_INVENTORY_ITEM_KEY`: the `Key` of a task inventory update that
/// carries an item.
pub const TASK_INVENTORY_ITEM_KEY: u8 = 0;
/// `TASK_INVENTORY_ASSET_KEY`: the `Key` of a task inventory update that
/// carries an asset.
pub const TASK_INVENTORY_ASSET_KEY: u8 = 1;

/// `MAX_INVENTORY_BUFFER_SIZE`, for `packBinaryBucket`.
pub const MAX_INVENTORY_BUFFER_SIZE: i32 = 1024;

/// `LLInventoryItemFlags::II_FLAGS_NONE`.
pub const II_FLAGS_NONE: u32 = 0;
/// The asset has only one reference in the system.
pub const II_FLAGS_SHARED_SINGLE_REFERENCE: u32 = 0x4000_0000;
/// A landmark has been visited.
pub const II_FLAGS_LANDMARK_VISITED: u32 = 1;
/// The next owner permissions are slammed on rez.
pub const II_FLAGS_OBJECT_SLAM_PERM: u32 = 0x100;
/// The sale information has been changed.
pub const II_FLAGS_OBJECT_SLAM_SALE: u32 = 0x1000;
pub const II_FLAGS_OBJECT_PERM_OVERWRITE_BASE: u32 = 0x01_0000;
pub const II_FLAGS_OBJECT_PERM_OVERWRITE_OWNER: u32 = 0x02_0000;
pub const II_FLAGS_OBJECT_PERM_OVERWRITE_GROUP: u32 = 0x04_0000;
pub const II_FLAGS_OBJECT_PERM_OVERWRITE_EVERYONE: u32 = 0x08_0000;
pub const II_FLAGS_OBJECT_PERM_OVERWRITE_NEXT_OWNER: u32 = 0x10_0000;
/// A returned object is composed of multiple items.
pub const II_FLAGS_OBJECT_HAS_MULTIPLE_ITEMS: u32 = 0x20_0000;
/// The low byte: the subtype of wearables and settings.
pub const II_FLAGS_SUBTYPE_MASK: u32 = 0x0000_00ff;
/// The bits cleared when the asset id of an existing item changes.
pub const II_FLAGS_PERM_OVERWRITE_MASK: u32 = II_FLAGS_OBJECT_SLAM_PERM
    | II_FLAGS_OBJECT_SLAM_SALE
    | II_FLAGS_OBJECT_PERM_OVERWRITE_BASE
    | II_FLAGS_OBJECT_PERM_OVERWRITE_OWNER
    | II_FLAGS_OBJECT_PERM_OVERWRITE_GROUP
    | II_FLAGS_OBJECT_PERM_OVERWRITE_EVERYONE
    | II_FLAGS_OBJECT_PERM_OVERWRITE_NEXT_OWNER;

// ---------------------------------------------------------------------------
// lltransactionflags.{h,cpp}
// ---------------------------------------------------------------------------

/// `TransactionFlags`.
pub type TransactionFlags = u8;

pub const TRANSACTION_FLAGS_NONE: TransactionFlags = 0;
pub const TRANSACTION_FLAG_SOURCE_GROUP: TransactionFlags = 1;
pub const TRANSACTION_FLAG_DEST_GROUP: TransactionFlags = 2;
pub const TRANSACTION_FLAG_OWNER_GROUP: TransactionFlags = 4;
pub const TRANSACTION_FLAG_SIMULTANEOUS_CONTRIBUTION: TransactionFlags = 8;
pub const TRANSACTION_FLAG_SIMULTANEOUS_CONTRIBUTION_REMOVAL: TransactionFlags = 16;

/// `pack_transaction_flags`.
pub fn pack_transaction_flags(is_source_group: bool, is_dest_group: bool) -> TransactionFlags {
    let mut rv = TRANSACTION_FLAGS_NONE;
    if is_source_group {
        rv |= TRANSACTION_FLAG_SOURCE_GROUP;
    }
    if is_dest_group {
        rv |= TRANSACTION_FLAG_DEST_GROUP;
    }
    rv
}

/// `is_tf_source_group`.
pub fn is_tf_source_group(flags: TransactionFlags) -> bool {
    flags & TRANSACTION_FLAG_SOURCE_GROUP == TRANSACTION_FLAG_SOURCE_GROUP
}

/// `is_tf_dest_group`.
pub fn is_tf_dest_group(flags: TransactionFlags) -> bool {
    flags & TRANSACTION_FLAG_DEST_GROUP == TRANSACTION_FLAG_DEST_GROUP
}

/// `is_tf_owner_group`.
pub fn is_tf_owner_group(flags: TransactionFlags) -> bool {
    flags & TRANSACTION_FLAG_OWNER_GROUP == TRANSACTION_FLAG_OWNER_GROUP
}

/// The transaction types (`lltransactiontypes.h`) the transfer messages
/// name.
pub const TRANS_GROUP_CREATE: i32 = 1002;
pub const TRANS_GROUP_JOIN: i32 = 1004;
pub const TRANS_UPLOAD_CHARGE: i32 = 1101;
pub const TRANS_OBJECT_SALE: i32 = 5000;
pub const TRANS_LAND_SALE: i32 = 5002;
pub const TRANS_LAND_PASS_SALE: i32 = 5006;
pub const TRANS_GROUP_LAND_DEED: i32 = 6001;

/// The currency symbol the messages carry by default (`Tea`'s `"L$"`).
pub const DEFAULT_CURRENCY: &[u8] = b"L$";

/// `Tea::wrapCurrency`: every `"L$"` of a fixed message text replaced by the
/// active currency symbol.
fn wrap_currency(out: &mut Vec<u8>, text: &str, currency: &[u8]) {
    let mut parts = text.split("L$");
    if let Some(first) = parts.next() {
        out.extend_from_slice(first.as_bytes());
    }
    for part in parts {
        out.extend_from_slice(currency);
        out.extend_from_slice(part.as_bytes());
    }
}

fn push_amount(out: &mut Vec<u8>, amount: i32) {
    out.extend_from_slice(amount.to_string().as_bytes());
}

/// `append_reason`: the reason clause of a transfer message.
fn append_reason(out: &mut Vec<u8>, transaction_type: i32, description: &[u8]) {
    match transaction_type {
        TRANS_OBJECT_SALE => {
            out.extend_from_slice(b" for ");
            out.extend_from_slice(if description.is_empty() { b"<unknown>" } else { description });
        }
        TRANS_LAND_SALE => out.extend_from_slice(b" for a parcel of land"),
        TRANS_LAND_PASS_SALE => out.extend_from_slice(b" for a land access pass"),
        TRANS_GROUP_LAND_DEED => out.extend_from_slice(b" for deeding land"),
        _ => {}
    }
}

/// `build_transfer_message_to_source`: the payer's message. `currency` is
/// the active currency symbol `Tea::wrapCurrency` substitutes for `L$`
/// ([`DEFAULT_CURRENCY`] unless an OpenSim grid or region sets another).
///
/// A null source, or a zero amount without description, gives the
/// description unchanged.
pub fn build_transfer_message_to_source(
    amount: i32,
    source_id: &[u8; 16],
    dest_id: &[u8; 16],
    dest_name: &[u8],
    transaction_type: i32,
    description: &[u8],
    currency: &[u8],
) -> Vec<u8> {
    if *source_id == [0; 16] || (amount == 0 && description.is_empty()) {
        return description.to_vec();
    }
    let mut out = Vec::new();
    if *dest_id == [0; 16] {
        wrap_currency(&mut out, "You paid L$", currency);
        push_amount(&mut out, amount);
        match transaction_type {
            TRANS_GROUP_CREATE => out.extend_from_slice(b" to create a group"),
            TRANS_GROUP_JOIN => out.extend_from_slice(b" to join a group"),
            TRANS_UPLOAD_CHARGE => out.extend_from_slice(b" to upload"),
            _ => {}
        }
    } else {
        out.extend_from_slice(b"You paid ");
        out.extend_from_slice(dest_name);
        wrap_currency(&mut out, " L$", currency);
        push_amount(&mut out, amount);
        append_reason(&mut out, transaction_type, description);
    }
    out.push(b'.');
    out
}

/// `build_transfer_message_to_destination`: the payee's message. A zero
/// amount gives the empty string, and a null destination the description
/// unchanged. `currency` is as for [`build_transfer_message_to_source`].
pub fn build_transfer_message_to_destination(
    amount: i32,
    dest_id: &[u8; 16],
    _source_id: &[u8; 16],
    source_name: &[u8],
    transaction_type: i32,
    description: &[u8],
    currency: &[u8],
) -> Vec<u8> {
    if amount == 0 {
        return Vec::new();
    }
    if *dest_id == [0; 16] {
        return description.to_vec();
    }
    let mut out = source_name.to_vec();
    wrap_currency(&mut out, " paid you L$", currency);
    push_amount(&mut out, amount);
    append_reason(&mut out, transaction_type, description);
    out.push(b'.');
    out
}
