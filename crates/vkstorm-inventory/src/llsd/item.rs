//! Item LLSD: `LLInventoryItem::asLLSD` and `fromLLSD`, and
//! `ll_create_sd_from_inventory_item` (`llinventory.cpp`).
//!
//! The inventory form is a map with `item_id`, `parent_id`, `permissions`
//! (the inventory permissions form), `asset_id` or `shadow_id`, `type` (the
//! asset type name), `inv_type` (the name, or an Integer for a type without
//! one), `flags` (4 bytes of Binary, big-endian), `sale_info`, `name`,
//! `desc`, `created_at`, and `thumbnail: {asset_id}`, `favorite: {toggled}`
//! and `script: {runtime}` when set.

use vkstorm_llsd::Value;

use super::coerce::{as_boolean, as_integer, as_string, as_uuid, get, has, insert, make_map, u32_from_sd};
use crate::item::Item;
use crate::legacy::reader::c_str;
use crate::perms::NULL_ID;
use crate::shadow::{exported_asset_id, unshadow_id};
use crate::text::{replace_nonstandard_ascii, sanitize_in_place};
use crate::types::{AssetType, InventoryType};

/// `INV_ITEM_ID_LABEL`.
pub const ITEM_ID: &str = "item_id";
/// `INV_PARENT_ID_LABEL`.
pub const PARENT_ID: &str = "parent_id";
/// `INV_THUMBNAIL_LABEL`.
pub const THUMBNAIL: &str = "thumbnail";
/// `INV_THUMBNAIL_ID_LABEL`: read only.
pub const THUMBNAIL_ID: &str = "thumbnail_id";
/// `INV_FAVORITE_LABEL`.
pub const FAVORITE: &str = "favorite";
/// `INV_TOGGLED_LABEL`.
pub const TOGGLED: &str = "toggled";
/// `INV_ASSET_TYPE_LABEL`.
pub const ASSET_TYPE: &str = "type";
/// `INV_INVENTORY_TYPE_LABEL`.
pub const INVENTORY_TYPE: &str = "inv_type";
/// `INV_NAME_LABEL`.
pub const NAME: &str = "name";
/// `INV_DESC_LABEL`.
pub const DESC: &str = "desc";
/// `INV_PERMISSIONS_LABEL`.
pub const PERMISSIONS: &str = "permissions";
/// `INV_SHADOW_ID_LABEL`.
pub const SHADOW_ID: &str = "shadow_id";
/// `INV_ASSET_ID_LABEL`.
pub const ASSET_ID: &str = "asset_id";
/// `INV_LINKED_ID_LABEL`: read only.
pub const LINKED_ID: &str = "linked_id";
/// `INV_SALE_INFO_LABEL`.
pub const SALE_INFO: &str = "sale_info";
/// `INV_FLAGS_LABEL`.
pub const FLAGS: &str = "flags";
/// `INV_CREATION_DATE_LABEL`.
pub const CREATION_DATE: &str = "created_at";

/// `LLSD().with(key, value)`: a one-key map.
pub(crate) fn with(key: &str, value: Value) -> Value {
    let mut sd = Value::map();
    insert(&mut sd, key, value);
    sd
}

/// `sd[key]` on a non-const `LLSD`: the entry, created undefined.
pub(crate) fn entry<'a>(sd: &'a mut Value, key: &str) -> &'a mut Value {
    make_map(sd).entry(key.as_bytes().to_vec()).or_insert(Value::Undefined)
}

impl Item {
    /// `LLInventoryItem::asLLSD()`: [`fill_llsd`](Self::fill_llsd) into a
    /// new map.
    pub fn to_llsd(&self) -> Value {
        let mut sd = Value::Undefined;
        self.fill_llsd(&mut sd);
        sd
    }

    /// `LLInventoryItem::asLLSD(LLSD& sd)`: writes the item's keys into `sd`
    /// without clearing it (a value that is not a map becomes one). The
    /// `permissions` and `sale_info` entries are filled in place, keeping
    /// any other keys they hold.
    ///
    /// The asset id goes under `asset_id` or `shadow_id` (see
    /// [`crate::shadow`]); `created_at` is the date's low 32 bits.
    pub fn fill_llsd(&self, sd: &mut Value) {
        let o = &self.object;
        insert(sd, ITEM_ID, Value::Uuid(o.id));
        insert(sd, PARENT_ID, Value::Uuid(o.parent));
        self.permissions.fill_llsd(entry(sd, PERMISSIONS));
        if o.thumbnail != NULL_ID {
            insert(sd, THUMBNAIL, with(ASSET_ID, Value::Uuid(o.thumbnail)));
        }
        if o.favorite {
            insert(sd, FAVORITE, with(TOGGLED, Value::Boolean(o.favorite)));
        }
        if !o.runtime.is_empty() {
            insert(sd, "script", with("runtime", Value::String(o.runtime.clone())));
        }
        let e = exported_asset_id(self.permissions.base, &self.asset);
        insert(sd, e.label(), Value::Uuid(*e.id()));
        insert(sd, ASSET_TYPE, Value::string(o.asset_type.lookup()));
        let inv_type = self.inv_type.lookup();
        if inv_type.is_empty() {
            insert(sd, INVENTORY_TYPE, Value::Integer(self.inv_type.0));
        } else {
            insert(sd, INVENTORY_TYPE, Value::string(inv_type));
        }
        // ll_sd_from_U32: the flags in network order.
        insert(sd, FLAGS, Value::Binary(self.flags.to_be_bytes().to_vec()));
        self.sale_info.fill_llsd(entry(sd, SALE_INFO));
        insert(sd, NAME, Value::String(o.name.clone()));
        insert(sd, DESC, Value::String(self.description.clone()));
        insert(sd, CREATION_DATE, Value::Integer(o.creation_date as i32));
    }

    /// `LLInventoryItem::fromLLSD(sd, is_new)`.
    ///
    /// `in_open_sim` is `LLPermissions::getIsInOpenSim()` (FIRE-36028):
    /// outside OpenSim the permissions are first reset to
    /// `init(null, null, null, null)`; in OpenSim the `permissions` map is
    /// read over the permissions the item has.
    ///
    /// - `is_new` sets the inventory type to `IT_NONE` and the asset id to
    ///   null first; the thumbnail is always nulled and the favorite
    ///   cleared.
    /// - The runtime is set (when `is_new`, or `script` or
    ///   `metadata.script` is present) from `script.runtime`, else
    ///   `metadata.script.runtime`.
    /// - Each key of the map, in key order: `item_id`, `parent_id`,
    ///   `thumbnail.asset_id`, `thumbnail_id`, `favorite.toggled`,
    ///   `permissions` (`importLLSD`), `sale_info` (a `perm_mask` in it
    ///   becomes the next-owner mask), `shadow_id` (unshadowed),
    ///   `asset_id`, `linked_id`, `type` and `inv_type` (a String by name, an
    ///   Integer as `(S8)(U8)`), `flags` (Binary by `ll_U32_from_sd`, or an
    ///   Integer), `name` (`replaceNonstandardASCII`, `replaceChar('|')`),
    ///   `desc` (`replaceNonstandardASCII`) and `created_at`
    ///   (`asInteger`). Other keys are ignored.
    /// - Then the inventory type repair of the readers and
    ///   `initMasks(inv_type)`.
    ///
    /// Returns true (`LLSaleInfo::fromLLSD` never fails).
    pub fn from_llsd(&mut self, sd: &Value, is_new: bool, in_open_sim: bool) -> bool {
        if is_new {
            self.inv_type = InventoryType::NONE;
            self.asset = NULL_ID;
        }
        self.object.thumbnail = NULL_ID;
        self.object.favorite = false;
        if !in_open_sim {
            self.permissions.init(&NULL_ID, &NULL_ID, &NULL_ID, &NULL_ID);
        }
        let metadata_script = has(sd, "metadata") && has(get(sd, "metadata"), "script");
        if is_new || has(sd, "script") || metadata_script {
            let script = if has(sd, "script") { get(sd, "script") } else { get(get(sd, "metadata"), "script") };
            self.object.runtime = as_string(get(script, "runtime"));
        }
        let Value::Map(map) = sd else {
            self.repair_inventory_type();
            return true;
        };
        for (label, v) in map {
            match label.as_slice() {
                b"item_id" => self.object.id = as_uuid(v),
                b"parent_id" => self.object.parent = as_uuid(v),
                b"thumbnail" => {
                    if has(v, ASSET_ID) {
                        self.object.thumbnail = as_uuid(get(v, ASSET_ID));
                    }
                }
                b"thumbnail_id" => self.object.thumbnail = as_uuid(v),
                b"favorite" => {
                    if has(v, TOGGLED) {
                        self.object.favorite = as_boolean(get(v, TOGGLED));
                    }
                }
                b"permissions" => self.permissions.import_llsd(v),
                b"sale_info" => {
                    let (mut has_perm_mask, mut perm_mask) = (false, 0u32);
                    if !self.sale_info.from_llsd(v, &mut has_perm_mask, &mut perm_mask) {
                        return false;
                    }
                    if has_perm_mask {
                        self.apply_legacy_perm_mask(perm_mask);
                    }
                }
                b"shadow_id" => self.asset = unshadow_id(&as_uuid(v)),
                b"asset_id" | b"linked_id" => self.asset = as_uuid(v),
                b"type" => match v {
                    Value::String(s) => self.object.asset_type = AssetType::from_name_cstr(Some(s)),
                    Value::Integer(i) => self.object.asset_type = AssetType(i32::from(*i as u8 as i8)),
                    _ => {}
                },
                b"inv_type" => match v {
                    Value::String(s) => self.inv_type = InventoryType::from_name(c_str(s)),
                    Value::Integer(i) => self.inv_type = InventoryType(i32::from(*i as u8 as i8)),
                    _ => {}
                },
                b"flags" => match v {
                    Value::Binary(_) => self.flags = u32_from_sd(v),
                    Value::Integer(i) => self.flags = *i as u32,
                    _ => {}
                },
                b"name" => {
                    let mut name = as_string(v);
                    sanitize_in_place(&mut name);
                    self.object.name = name;
                }
                b"desc" => {
                    let mut desc = as_string(v);
                    replace_nonstandard_ascii(&mut desc, b' ');
                    self.description = desc;
                }
                b"created_at" => self.object.creation_date = i64::from(as_integer(v)),
                _ => {}
            }
        }
        self.repair_inventory_type();
        true
    }

    /// `ll_create_sd_from_inventory_item`: undefined for an `AT_NONE` item
    /// (the C++ logs a warning); else a map of the id, parent, name, type
    /// name, the asset id (never shadowed), description,
    /// `ll_create_sd_from_sale_info`, `ll_create_sd_from_permissions`, the
    /// inventory type name (empty for a type without one), the flags as an
    /// Integer and the date's low 32 bits.
    pub fn create_sd(&self) -> Value {
        let o = &self.object;
        if o.asset_type == AssetType::NONE {
            return Value::Undefined;
        }
        let mut rv = Value::map();
        insert(&mut rv, ITEM_ID, Value::Uuid(o.id));
        insert(&mut rv, PARENT_ID, Value::Uuid(o.parent));
        insert(&mut rv, NAME, Value::String(o.name.clone()));
        insert(&mut rv, ASSET_TYPE, Value::string(o.asset_type.lookup()));
        insert(&mut rv, ASSET_ID, Value::Uuid(self.asset));
        insert(&mut rv, DESC, Value::String(self.description.clone()));
        insert(&mut rv, SALE_INFO, self.sale_info.to_llsd());
        insert(&mut rv, PERMISSIONS, self.permissions.to_llsd());
        insert(&mut rv, INVENTORY_TYPE, Value::string(self.inv_type.lookup()));
        insert(&mut rv, FLAGS, Value::Integer(self.flags as i32));
        insert(&mut rv, CREATION_DATE, Value::Integer(o.creation_date as i32));
        rv
    }
}
