//! Permissions LLSD: `LLPermissions::importLLSD`, `ll_create_sd_from_permissions`
//! and `ll_permissions_from_sd` (`llpermissions.cpp`).
//!
//! Two dialects:
//! - the inventory form (`creator_id`, ..., `is_owner_group`, `base_mask`,
//!   ...), written by `ll_fill_sd_from_permissions` and read by
//!   `importLLSD`, which never reads `is_owner_group`;
//! - the hyphenated message form (`creator-id`, ..., `group-owned`) of
//!   `packMessage()`/`unpackMessage(LLSD)`, which no viewer code calls.

use vkstorm_llsd::Value;

use super::coerce::{as_boolean, as_integer, as_uuid, get, insert};
use crate::perms::{Permissions, Uuid};

/// `PERM_CREATOR_ID_LABEL`.
pub const CREATOR_ID: &str = "creator_id";
/// `PERM_OWNER_ID_LABEL`.
pub const OWNER_ID: &str = "owner_id";
/// `PERM_LAST_OWNER_ID_LABEL`.
pub const LAST_OWNER_ID: &str = "last_owner_id";
/// `PERM_GROUP_ID_LABEL`.
pub const GROUP_ID: &str = "group_id";
/// `PERM_IS_OWNER_GROUP_LABEL`: written, never read.
pub const IS_OWNER_GROUP: &str = "is_owner_group";
/// `PERM_BASE_MASK_LABEL`.
pub const BASE_MASK: &str = "base_mask";
/// `PERM_OWNER_MASK_LABEL`.
pub const OWNER_MASK: &str = "owner_mask";
/// `PERM_GROUP_MASK_LABEL`.
pub const GROUP_MASK: &str = "group_mask";
/// `PERM_EVERYONE_MASK_LABEL`.
pub const EVERYONE_MASK: &str = "everyone_mask";
/// `PERM_NEXT_OWNER_MASK_LABEL`.
pub const NEXT_OWNER_MASK: &str = "next_owner_mask";

fn uuid(id: &Uuid) -> Value {
    Value::Uuid(*id)
}

/// A mask as `(LLSD::Integer)`: its bits as a signed integer.
fn mask(m: u32) -> Value {
    Value::Integer(m as i32)
}

impl Permissions {
    /// `ll_create_sd_from_permissions`: a new map filled by
    /// [`fill_llsd`](Self::fill_llsd).
    pub fn to_llsd(&self) -> Value {
        let mut sd = Value::Undefined;
        self.fill_llsd(&mut sd);
        sd
    }

    /// `ll_fill_sd_from_permissions`: sets the ten keys of the inventory
    /// form in `sd`, keeping its other keys. A value that is not a map
    /// becomes one first (`LLSD::operator[]`).
    pub fn fill_llsd(&self, sd: &mut Value) {
        insert(sd, CREATOR_ID, uuid(&self.creator));
        insert(sd, OWNER_ID, uuid(&self.owner));
        insert(sd, LAST_OWNER_ID, uuid(&self.last_owner));
        insert(sd, GROUP_ID, uuid(&self.group));
        insert(sd, IS_OWNER_GROUP, Value::Boolean(self.group_owned));
        insert(sd, BASE_MASK, mask(self.base));
        insert(sd, OWNER_MASK, mask(self.owner_mask));
        insert(sd, GROUP_MASK, mask(self.group_mask));
        insert(sd, EVERYONE_MASK, mask(self.everyone));
        insert(sd, NEXT_OWNER_MASK, mask(self.next));
    }

    /// `LLPermissions::importLLSD`: over the record as it is, each known key
    /// of a map sets its field (`asUUID`, masks `asInteger`); other keys,
    /// `is_owner_group` included, and non-map input are ignored. Then
    /// `fixOwnership()` (the flag follows the ids alone) and `fix()`.
    pub fn import_llsd(&mut self, sd: &Value) {
        if let Value::Map(m) = sd {
            for (label, v) in m {
                match label.as_slice() {
                    b"creator_id" => self.creator = as_uuid(v),
                    b"owner_id" => self.owner = as_uuid(v),
                    b"last_owner_id" => self.last_owner = as_uuid(v),
                    b"group_id" => self.group = as_uuid(v),
                    b"base_mask" => self.base = as_integer(v) as u32,
                    b"owner_mask" => self.owner_mask = as_integer(v) as u32,
                    b"everyone_mask" => self.everyone = as_integer(v) as u32,
                    b"group_mask" => self.group_mask = as_integer(v) as u32,
                    b"next_owner_mask" => self.next = as_integer(v) as u32,
                    _ => {}
                }
            }
        }
        self.fix_ownership();
        self.fix();
    }

    /// `ll_permissions_from_sd`: [`import_llsd`](Self::import_llsd) over
    /// the default record.
    pub fn from_llsd(sd: &Value) -> Permissions {
        let mut p = Permissions::DEFAULT;
        p.import_llsd(sd);
        p
    }

    /// `LLPermissions::packMessage()` (LLSD): the hyphenated form, without
    /// the last owner; masks as signed integers, `group-owned` a boolean.
    pub fn pack_message_llsd(&self) -> Value {
        let mut sd = Value::map();
        insert(&mut sd, "creator-id", uuid(&self.creator));
        insert(&mut sd, "owner-id", uuid(&self.owner));
        insert(&mut sd, "group-id", uuid(&self.group));
        insert(&mut sd, "base-mask", mask(self.base));
        insert(&mut sd, "owner-mask", mask(self.owner_mask));
        insert(&mut sd, "group-mask", mask(self.group_mask));
        insert(&mut sd, "everyone-mask", mask(self.everyone));
        insert(&mut sd, "next-owner-mask", mask(self.next));
        insert(&mut sd, "group-owned", Value::Boolean(self.group_owned));
        sd
    }

    /// `LLPermissions::unpackMessage(LLSD)`: every field of the hyphenated
    /// form, raw (a missing key reads as undefined: null, 0, false); the
    /// last owner is kept and nothing is fixed.
    pub fn unpack_message_llsd(&mut self, sd: &Value) {
        self.creator = as_uuid(get(sd, "creator-id"));
        self.owner = as_uuid(get(sd, "owner-id"));
        self.group = as_uuid(get(sd, "group-id"));
        self.base = as_integer(get(sd, "base-mask")) as u32;
        self.owner_mask = as_integer(get(sd, "owner-mask")) as u32;
        self.group_mask = as_integer(get(sd, "group-mask")) as u32;
        self.everyone = as_integer(get(sd, "everyone-mask")) as u32;
        self.next = as_integer(get(sd, "next-owner-mask")) as u32;
        self.group_owned = as_boolean(get(sd, "group-owned"));
    }
}
