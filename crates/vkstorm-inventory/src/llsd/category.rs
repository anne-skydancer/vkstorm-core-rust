//! Category LLSD: `LLInventoryCategory::asLLSD`, `asAISCreateCatLLSD`, `fromLLSD`,
//! `exportLLSD`, `importLLSDMap`/`importLLSD` and the `ll_create_*` helpers (`llinventory.cpp`),
//! and `LLViewerInventoryCategory::exportLLSD`/`importLLSD` (`llviewerinventory.cpp`).
//!
//! Four dialects, by their id and type keys:
//! 1. `asLLSD`: `item_id`, `type` = the preferred type as an `(S8)` Integer;
//! 2. `asAISCreateCatLLSD`: `category_id`, `type_default` = the same;
//! 3. `fromLLSD` reads `category_id` (never `item_id`), `type` and
//!    `type_default`;
//! 4. `exportLLSD`/`importLLSD`: `cat_id`, `type` = the asset type name,
//!    `preferred_type` = the folder type name; the viewer subclass adds
//!    `owner_id` and `version`.

use vkstorm_llsd::Value;

use super::coerce::{as_boolean, as_integer, as_string, as_uuid, get, has, insert};
use super::item::{with, ASSET_ID, ASSET_TYPE, FAVORITE, NAME, PARENT_ID, THUMBNAIL, THUMBNAIL_ID, TOGGLED};
use crate::category::{Category, ViewerCategory};
use crate::perms::NULL_ID;
use crate::text::sanitize_in_place;
use crate::types::{AssetType, FolderType};

/// `INV_FOLDER_ID_LABEL`.
pub const FOLDER_ID: &str = "cat_id";
/// `INV_PREFERRED_TYPE_LABEL`.
pub const PREFERRED_TYPE: &str = "preferred_type";
/// `INV_ASSET_TYPE_LABEL_WS` (agent inventory service).
pub const ASSET_TYPE_WS: &str = "type_default";
/// `INV_FOLDER_ID_LABEL_WS` (agent inventory service).
pub const FOLDER_ID_WS: &str = "category_id";
/// `INV_ITEM_ID_LABEL`, the id key of `asLLSD`.
pub const ITEM_ID: &str = "item_id";
/// `INV_OWNER_ID` (`llviewerinventory.cpp`).
pub const OWNER_ID: &str = "owner_id";
/// `INV_VERSION` (`llviewerinventory.cpp`).
pub const VERSION: &str = "version";

/// `(S8)type` of an `S8`-typed write: the low byte, sign-extended.
fn s8(v: i32) -> i32 {
    i32::from(v as i8)
}

impl Category {
    /// The keys `asLLSD` and `asAISCreateCatLLSD` share after the id and
    /// type: `parent_id`, `name`, and the thumbnail and favorite maps when
    /// set.
    fn fill_common(&self, sd: &mut Value) {
        let o = &self.object;
        insert(sd, PARENT_ID, Value::Uuid(o.parent));
        insert(sd, NAME, Value::String(o.name.clone()));
        if o.thumbnail != NULL_ID {
            insert(sd, THUMBNAIL, with(ASSET_ID, Value::Uuid(o.thumbnail)));
        }
        if o.favorite {
            insert(sd, FAVORITE, with(TOGGLED, Value::Boolean(true)));
        }
    }

    /// `LLInventoryCategory::asLLSD` (dialect 1): `item_id`, and `type` as
    /// the preferred type's `(S8)` Integer.
    pub fn to_llsd(&self) -> Value {
        let mut sd = Value::map();
        insert(&mut sd, ITEM_ID, Value::Uuid(self.object.id));
        insert(&mut sd, ASSET_TYPE, Value::Integer(s8(self.preferred_type.0)));
        self.fill_common(&mut sd);
        sd
    }

    /// `asAISCreateCatLLSD` (dialect 2): `category_id`, and `type_default`
    /// as the preferred type's `(S8)` Integer.
    pub fn to_ais_create_llsd(&self) -> Value {
        let mut sd = Value::map();
        insert(&mut sd, FOLDER_ID_WS, Value::Uuid(self.object.id));
        insert(&mut sd, ASSET_TYPE_WS, Value::Integer(s8(self.preferred_type.0)));
        self.fill_common(&mut sd);
        sd
    }

    /// `LLInventoryCategory::fromLLSD` (dialect 3), over the category as it
    /// is:
    /// - `category_id` and `parent_id` set the ids (`asUUID`);
    /// - the thumbnail is nulled, then set from `thumbnail.asset_id`, or
    ///   from `thumbnail_id` when there is no `thumbnail` key;
    /// - the favorite is cleared, then set from `favorite.toggled`;
    /// - `type`, then `type_default`, set the preferred type as `(S8)(U8)`
    ///   of `asInteger()`;
    /// - `name` sets the name (`replaceNonstandardASCII`,
    ///   `replaceChar('|')`).
    ///
    /// The asset type, date and runtime are kept. Always returns true.
    pub fn from_llsd(&mut self, sd: &Value) -> bool {
        if has(sd, FOLDER_ID_WS) {
            self.object.id = as_uuid(get(sd, FOLDER_ID_WS));
        }
        if has(sd, PARENT_ID) {
            self.object.parent = as_uuid(get(sd, PARENT_ID));
        }
        self.object.thumbnail = NULL_ID;
        if has(sd, THUMBNAIL) {
            let t = get(sd, THUMBNAIL);
            if has(t, ASSET_ID) {
                self.object.thumbnail = as_uuid(get(t, ASSET_ID));
            }
        } else if has(sd, THUMBNAIL_ID) {
            self.object.thumbnail = as_uuid(get(sd, THUMBNAIL_ID));
        }
        self.object.favorite = false;
        if has(sd, FAVORITE) {
            let f = get(sd, FAVORITE);
            if has(f, TOGGLED) {
                self.object.favorite = as_boolean(get(f, TOGGLED));
            }
        }
        for key in [ASSET_TYPE, ASSET_TYPE_WS] {
            if has(sd, key) {
                self.preferred_type = FolderType(s8(as_integer(get(sd, key))));
            }
        }
        if has(sd, NAME) {
            let mut name = as_string(get(sd, NAME));
            sanitize_in_place(&mut name);
            self.object.name = name;
        }
        true
    }

    /// `LLInventoryCategory::exportLLSD` (dialect 4): writes `cat_id`,
    /// `parent_id`, `type` (the asset type name), `preferred_type` (the
    /// folder type name), `name`, and the thumbnail and favorite maps when
    /// set, into `sd` without clearing it.
    pub fn export_llsd(&self, sd: &mut Value) {
        let o = &self.object;
        insert(sd, FOLDER_ID, Value::Uuid(o.id));
        insert(sd, PARENT_ID, Value::Uuid(o.parent));
        insert(sd, ASSET_TYPE, Value::string(o.asset_type.lookup()));
        insert(sd, PREFERRED_TYPE, Value::string(self.preferred_type.lookup()));
        insert(sd, NAME, Value::String(o.name.clone()));
        if o.thumbnail != NULL_ID {
            insert(sd, THUMBNAIL, with(ASSET_ID, Value::Uuid(o.thumbnail)));
        }
        if o.favorite {
            insert(sd, FAVORITE, with(TOGGLED, Value::Boolean(true)));
        }
    }

    /// `LLInventoryCategory::importLLSD(label, value)`: one key of dialect
    /// 4. `cat_id` and `parent_id` set the ids; `type` calls `setType` with
    /// the asset type of `asString()` (a type other than `AT_LSL_TEXT`
    /// clears the runtime); `preferred_type` sets the folder type by name;
    /// `thumbnail` sets the thumbnail to its `asset_id` (null without);
    /// `name` sets the name (`replaceNonstandardASCII`, `replaceChar('|')`).
    /// These return true.
    ///
    /// `favorite` sets the favorite to its `toggled` (false without) but
    /// returns false, as the C++ branch has no `return true`. Any other key
    /// returns false.
    pub fn import_llsd(&mut self, label: &[u8], value: &Value) -> bool {
        match label {
            b"cat_id" => self.object.id = as_uuid(value),
            b"parent_id" => self.object.parent = as_uuid(value),
            b"type" => self.object.set_type(AssetType::from_name(&as_string(value))),
            b"preferred_type" => self.preferred_type = FolderType::from_name(&as_string(value)),
            b"thumbnail" => {
                self.object.thumbnail = if has(value, ASSET_ID) { as_uuid(get(value, ASSET_ID)) } else { NULL_ID };
            }
            b"favorite" => {
                self.object.favorite = has(value, TOGGLED) && as_boolean(get(value, TOGGLED));
                return false;
            }
            b"name" => {
                let mut name = as_string(value);
                sanitize_in_place(&mut name);
                self.object.name = name;
            }
            _ => return false,
        }
        true
    }

    /// `LLInventoryCategory::importLLSDMap`: [`import_llsd`](Self::import_llsd)
    /// for each key of a map, in key order. Always returns true.
    pub fn import_llsd_map(&mut self, sd: &Value) -> bool {
        if let Value::Map(m) = sd {
            for (label, value) in m {
                self.import_llsd(label, value);
            }
        }
        true
    }

    /// `ll_create_sd_from_inventory_category`: undefined for an `AT_NONE`
    /// category (the C++ logs a warning); else `cat_id`, `parent_id`,
    /// `name`, `type` (the asset type name), and `preferred_type` (the
    /// folder type name) only for a protected preferred type.
    pub fn create_sd(&self) -> Value {
        let o = &self.object;
        if o.asset_type == AssetType::NONE {
            return Value::Undefined;
        }
        let mut rv = Value::map();
        insert(&mut rv, FOLDER_ID, Value::Uuid(o.id));
        insert(&mut rv, PARENT_ID, Value::Uuid(o.parent));
        insert(&mut rv, NAME, Value::String(o.name.clone()));
        insert(&mut rv, ASSET_TYPE, Value::string(o.asset_type.lookup()));
        if self.preferred_type.is_protected() {
            insert(&mut rv, PREFERRED_TYPE, Value::string(self.preferred_type.lookup()));
        }
        rv
    }

    /// `ll_create_category_from_sd`: a default category, then
    /// `setUUID(cat_id)`, `setParent(parent_id)`, `rename(name)`,
    /// `setType` by the name of `type` and `setPreferredType` by the name of
    /// `preferred_type` (every value by `asUUID`/`asString`, so a missing
    /// key reads as null or empty).
    pub fn from_create_sd(sd: &Value) -> Category {
        let mut c = Category::default();
        c.object.id = as_uuid(get(sd, FOLDER_ID));
        c.object.parent = as_uuid(get(sd, PARENT_ID));
        c.object.rename(&as_string(get(sd, NAME)));
        c.object.set_type(AssetType::from_name(&as_string(get(sd, ASSET_TYPE))));
        c.preferred_type = FolderType::from_name(&as_string(get(sd, PREFERRED_TYPE)));
        c
    }
}

impl ViewerCategory {
    /// `LLViewerInventoryCategory::exportLLSD`: the base
    /// [`Category::export_llsd`], then `owner_id` and `version`.
    pub fn export_llsd(&self, sd: &mut Value) {
        self.category.export_llsd(sd);
        insert(sd, OWNER_ID, Value::Uuid(self.owner));
        insert(sd, VERSION, Value::Integer(self.version));
    }

    /// `LLViewerInventoryCategory::importLLSD`: the base
    /// [`Category::import_llsd`]; when that returns false, `owner_id`
    /// (`asUUID`) and `version` (`setVersion(asInteger())`) return true.
    pub fn import_llsd(&mut self, label: &[u8], value: &Value) -> bool {
        if self.category.import_llsd(label, value) {
            return true;
        }
        match label {
            b"owner_id" => self.owner = as_uuid(value),
            b"version" => self.version = as_integer(value),
            _ => return false,
        }
        true
    }

    /// `importLLSDMap` on the viewer subclass (`importLLSD` is virtual):
    /// [`import_llsd`](Self::import_llsd) for each key, in key order. Always
    /// returns true.
    pub fn import_llsd_map(&mut self, sd: &Value) -> bool {
        if let Value::Map(m) = sd {
            for (label, value) in m {
                self.import_llsd(label, value);
            }
        }
        true
    }
}
